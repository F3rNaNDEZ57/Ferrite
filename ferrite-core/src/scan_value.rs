//! Typed scan values and the comparison predicates the scan engine uses.
//! Deliberately free of any I/O — these are pure functions, unit-testable
//! with no process involved.

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

/// A value the scan engine can search for or compare — the numeric types in
/// v1's scan matrix (see the vault's `v1-scope.md`).
///
/// Byte-pattern (AOB) scanning is a separate code path, not a variant here:
/// it's a substring search over arbitrary-length byte patterns, mechanically
/// different from a fixed-width numeric compare, and doesn't belong
/// pretending to be a numeric value.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ScanValue {
    I8(i8),
    I16(i16),
    I32(i32),
    I64(i64),
    F32(f32),
    F64(f64),
}

impl ScanValue {
    /// Size in bytes of this value's type.
    pub fn size(self) -> usize {
        match self {
            Self::I8(_) => 1,
            Self::I16(_) => 2,
            Self::I32(_) | Self::F32(_) => 4,
            Self::I64(_) | Self::F64(_) => 8,
        }
    }

    /// This value's little-endian byte representation.
    pub fn to_le_bytes(self) -> Vec<u8> {
        match self {
            Self::I8(v) => v.to_le_bytes().to_vec(),
            Self::I16(v) => v.to_le_bytes().to_vec(),
            Self::I32(v) => v.to_le_bytes().to_vec(),
            Self::I64(v) => v.to_le_bytes().to_vec(),
            Self::F32(v) => v.to_le_bytes().to_vec(),
            Self::F64(v) => v.to_le_bytes().to_vec(),
        }
    }

    /// Reinterprets `bytes` as this value's type — used by the scanner to
    /// turn a freshly-read memory slice into a value of the same type as
    /// `self`, so the two can be compared.
    ///
    /// # Panics
    /// Panics if `bytes.len()` doesn't equal `self.size()`. That's a scanner
    /// bug (it read the wrong number of bytes for the type it's scanning),
    /// not a runtime/data condition — a panic is the right response.
    pub fn from_le_bytes_like(self, bytes: &[u8]) -> Self {
        match self {
            Self::I8(_) => Self::I8(i8::from_le_bytes(
                bytes
                    .try_into()
                    .expect("byte slice size must match type size"),
            )),
            Self::I16(_) => Self::I16(i16::from_le_bytes(
                bytes
                    .try_into()
                    .expect("byte slice size must match type size"),
            )),
            Self::I32(_) => Self::I32(i32::from_le_bytes(
                bytes
                    .try_into()
                    .expect("byte slice size must match type size"),
            )),
            Self::I64(_) => Self::I64(i64::from_le_bytes(
                bytes
                    .try_into()
                    .expect("byte slice size must match type size"),
            )),
            Self::F32(_) => Self::F32(f32::from_le_bytes(
                bytes
                    .try_into()
                    .expect("byte slice size must match type size"),
            )),
            Self::F64(_) => Self::F64(f64::from_le_bytes(
                bytes
                    .try_into()
                    .expect("byte slice size must match type size"),
            )),
        }
    }

    /// False for a non-finite float (NaN/infinity) — common when garbage
    /// memory is reinterpreted as a float. Always true for integers.
    ///
    /// Non-finite values are excluded from every filter (see
    /// [`passes_filter`]) rather than participating with NaN's always-false
    /// comparisons, which would otherwise silently and confusingly drop
    /// entries from `changed`/increased`/`decreased` results.
    pub fn is_finite(self) -> bool {
        match self {
            Self::F32(v) => v.is_finite(),
            Self::F64(v) => v.is_finite(),
            _ => true,
        }
    }

    fn ordering_against(self, other: Self) -> Ordering {
        match (self, other) {
            (Self::I8(a), Self::I8(b)) => a.cmp(&b),
            (Self::I16(a), Self::I16(b)) => a.cmp(&b),
            (Self::I32(a), Self::I32(b)) => a.cmp(&b),
            (Self::I64(a), Self::I64(b)) => a.cmp(&b),
            (Self::F32(a), Self::F32(b)) => a
                .partial_cmp(&b)
                .expect("non-finite values must be filtered out before comparing"),
            (Self::F64(a), Self::F64(b)) => a
                .partial_cmp(&b)
                .expect("non-finite values must be filtered out before comparing"),
            _ => panic!("ordering_against called with mismatched ScanValue variants"),
        }
    }
}

/// A next-scan filter, applied against a previously-matched address's old
/// and freshly-read new value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanFilter {
    Changed,
    Unchanged,
    Increased,
    Decreased,
}

/// Whether `new` passes `filter` relative to `old` (the value recorded at
/// this address by the previous scan). `old` and `new` must be the same
/// [`ScanValue`] variant — the scanner always re-reads at the type it's
/// scanning, so this holds by construction.
///
/// Changed/unchanged compare bitwise, deliberately with no epsilon: a real,
/// tiny change (a health value ticking down) is still a change, and an
/// epsilon here would hide it from the user asking "did this change?". Any
/// value that's non-finite (NaN/infinity, common in garbage memory
/// reinterpreted as a float) never passes any filter.
pub fn passes_filter(old: ScanValue, new: ScanValue, filter: ScanFilter) -> bool {
    if !old.is_finite() || !new.is_finite() {
        return false;
    }
    match filter {
        ScanFilter::Changed => old != new,
        ScanFilter::Unchanged => old == new,
        ScanFilter::Increased => old.ordering_against(new) == Ordering::Less,
        ScanFilter::Decreased => old.ordering_against(new) == Ordering::Greater,
    }
}

/// Whether `bytes` (exactly `target.size()` long) matches `target`, for an
/// exact-value first scan.
///
/// Integers compare as a literal byte pattern — a byte search, no decode
/// needed, which keeps the scan's hot loop cheap. Floats decode both sides
/// and compare rounded to `float_decimals` decimal places: the value the
/// user typed and the value actually stored can differ in raw bits while
/// being identical at the precision the user cares about. Bytes that decode
/// to a non-finite float never match.
pub fn bytes_match_exact(bytes: &[u8], target: ScanValue, float_decimals: u32) -> bool {
    match target {
        ScanValue::F32(_) | ScanValue::F64(_) => {
            let candidate = target.from_le_bytes_like(bytes);
            candidate.is_finite() && floats_equal_rounded(candidate, target, float_decimals)
        }
        _ => bytes == target.to_le_bytes().as_slice(),
    }
}

fn floats_equal_rounded(a: ScanValue, b: ScanValue, decimals: u32) -> bool {
    let factor = 10f64.powi(decimals as i32);
    match (a, b) {
        (ScanValue::F32(x), ScanValue::F32(y)) => {
            (f64::from(x) * factor).round() == (f64::from(y) * factor).round()
        }
        (ScanValue::F64(x), ScanValue::F64(y)) => (x * factor).round() == (y * factor).round(),
        _ => unreachable!("floats_equal_rounded called with a non-float ScanValue"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_int_match_is_a_literal_byte_pattern() {
        let target = ScanValue::I32(100);
        assert!(bytes_match_exact(&100i32.to_le_bytes(), target, 2));
        assert!(!bytes_match_exact(&101i32.to_le_bytes(), target, 2));
    }

    #[test]
    fn exact_float_match_tolerates_bit_level_noise_within_rounding() {
        let target = ScanValue::F32(12.34);
        // A value that displays the same at 2 decimals but isn't bit-identical.
        let noisy = 12.340_001_f32;
        assert!(bytes_match_exact(&noisy.to_le_bytes(), target, 2));

        let different = 12.36_f32;
        assert!(!bytes_match_exact(&different.to_le_bytes(), target, 2));
    }

    #[test]
    fn exact_float_match_rejects_non_finite_bytes() {
        let target = ScanValue::F32(12.34);
        assert!(!bytes_match_exact(&f32::NAN.to_le_bytes(), target, 2));
    }

    #[test]
    fn changed_and_unchanged_are_bitwise_not_epsilon() {
        // A change too small to matter at typical display precision must
        // still register as Changed - no epsilon hides it here.
        let old = ScanValue::F32(1.000_000_0);
        let new = ScanValue::F32(1.000_000_1);
        assert!(passes_filter(old, new, ScanFilter::Changed));
        assert!(!passes_filter(old, new, ScanFilter::Unchanged));
    }

    #[test]
    fn increased_and_decreased_for_integers() {
        let old = ScanValue::I32(100);
        let higher = ScanValue::I32(150);
        let lower = ScanValue::I32(50);

        assert!(passes_filter(old, higher, ScanFilter::Increased));
        assert!(!passes_filter(old, higher, ScanFilter::Decreased));
        assert!(passes_filter(old, lower, ScanFilter::Decreased));
        assert!(!passes_filter(old, lower, ScanFilter::Increased));
    }

    #[test]
    fn non_finite_values_never_pass_any_filter() {
        let nan = ScanValue::F32(f32::NAN);
        let normal = ScanValue::F32(1.0);

        for filter in [
            ScanFilter::Changed,
            ScanFilter::Unchanged,
            ScanFilter::Increased,
            ScanFilter::Decreased,
        ] {
            assert!(!passes_filter(nan, normal, filter));
            assert!(!passes_filter(normal, nan, filter));
        }
    }
}
