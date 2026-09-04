//! Byte-pattern ("array of bytes" / AOB) scanning: a substring search over
//! memory, mechanically distinct from [`crate::scan_value::ScanValue`]'s
//! fixed-width numeric compare. No alignment requirement — a pattern can
//! start at any address, unlike a typed numeric value.

use crate::regions::MemoryRegion;
use crate::scan::ScanOptions;
use crate::session::ProcessSession;

/// Parses a hex byte pattern like `"3F A2 01"` or `"3FA201"` (whitespace
/// optional, case-insensitive) into raw bytes.
pub fn parse_hex_pattern(text: &str) -> Result<Vec<u8>, String> {
    let cleaned: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    if cleaned.is_empty() {
        return Err("enter a byte pattern, e.g. 3F A2 01".to_string());
    }
    if !cleaned.len().is_multiple_of(2) {
        return Err("byte pattern must have an even number of hex digits".to_string());
    }

    let mut bytes = Vec::with_capacity(cleaned.len() / 2);
    for chunk in cleaned.as_bytes().chunks(2) {
        let pair = std::str::from_utf8(chunk).expect("ASCII hex digits are valid UTF-8");
        let byte =
            u8::from_str_radix(pair, 16).map_err(|_| format!("'{pair}' isn't a valid hex byte"))?;
        bytes.push(byte);
    }
    Ok(bytes)
}

/// Formats bytes back as a hex pattern string (e.g. `[0x3F, 0xA2]` ->
/// `"3F A2"`) — the same format [`parse_hex_pattern`] accepts, so a result
/// can be copied back into the search box.
pub fn format_pattern(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// One matched address from an AOB scan, with the bytes found there and the
/// ones it held before the most recent re-read.
#[derive(Debug, Clone, PartialEq)]
pub struct AobMatch {
    pub address: usize,
    pub bytes: Vec<u8>,
    /// What `bytes` were before the last [`next_scan_aob`], for the same
    /// reason [`crate::scan::ScanMatch::previous`] exists — equal to `bytes`
    /// on a first scan.
    pub previous: Vec<u8>,
}

/// Result of an AOB first scan. `capped` mirrors
/// [`crate::scan::FirstScanResult`]'s meaning.
#[derive(Debug, Clone, PartialEq)]
pub struct AobScanResult {
    pub matches: Vec<AobMatch>,
    pub capped: bool,
}

/// A next-scan filter for AOB matches. No Increased/Decreased — meaningless
/// for a byte pattern, per the vault's `v1-scope.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AobFilter {
    Changed,
    Unchanged,
}

/// Performs an exact-pattern first scan across every writable region.
pub fn first_scan_aob(
    session: &ProcessSession,
    pattern: &[u8],
    options: ScanOptions,
) -> AobScanResult {
    let mut matches = Vec::new();

    for region in session.writable_regions() {
        let remaining_budget = options.max_results - matches.len();
        if remaining_budget == 0 {
            break;
        }
        matches.extend(scan_region_aob(
            session,
            region,
            pattern,
            options,
            remaining_budget,
        ));
    }

    let capped = matches.len() >= options.max_results;
    AobScanResult { matches, capped }
}

/// Scans a single region for `pattern`, returning at most `max_matches`.
/// Mirrors [`crate::scan::scan_region_exact`]'s chunk-overlap approach, but
/// the overlap is `pattern.len() - 1` (a runtime-determined length, unlike a
/// fixed 1–8 byte numeric type), and there's no alignment step — every byte
/// offset is a candidate start position.
pub fn scan_region_aob(
    session: &ProcessSession,
    region: MemoryRegion,
    pattern: &[u8],
    options: ScanOptions,
    max_matches: usize,
) -> Vec<AobMatch> {
    if pattern.is_empty() {
        return Vec::new();
    }

    let overlap = pattern.len().saturating_sub(1);
    let mut matches = Vec::new();
    let mut offset = 0usize;

    while offset < region.size {
        let remaining = region.size - offset;
        let read_len = remaining.min(options.chunk_size + overlap);
        let chunk_address = region.base_address + offset;

        if let Ok(bytes) = session.read_bytes(chunk_address, read_len) {
            for m in scan_bytes_for_pattern(chunk_address, &bytes, pattern) {
                matches.push(m);
                if matches.len() >= max_matches {
                    return matches;
                }
            }
        }

        offset += options.chunk_size;
    }

    matches
}

/// Substring-searches `bytes` (read from `base_address`) for every
/// occurrence of `pattern`. Pure and process-free.
fn scan_bytes_for_pattern(base_address: usize, bytes: &[u8], pattern: &[u8]) -> Vec<AobMatch> {
    let mut matches = Vec::new();
    if pattern.is_empty() || bytes.len() < pattern.len() {
        return matches;
    }

    for start in 0..=(bytes.len() - pattern.len()) {
        if &bytes[start..start + pattern.len()] == pattern {
            matches.push(AobMatch {
                address: base_address + start,
                bytes: pattern.to_vec(),
                // Nothing has changed yet on a first scan.
                previous: pattern.to_vec(),
            });
        }
    }
    matches
}

/// Re-checks previously matched AOB addresses against `filter`. Mirrors
/// [`crate::scan::next_scan`]: dumb discrete re-reads (not chunked — this is
/// a list of specific addresses, not a memory range), the stored bytes are
/// updated on every surviving match (same staleness reasoning as the
/// numeric path — a second `Changed` scan must compare against what the
/// first one actually found, not the original pattern), and a match whose
/// address fails to re-read is dropped, not retained with stale bytes.
pub fn next_scan_aob(
    session: &ProcessSession,
    matches: &[AobMatch],
    filter: AobFilter,
) -> Vec<AobMatch> {
    matches
        .iter()
        .filter_map(|m| {
            let bytes = session.read_bytes(m.address, m.bytes.len()).ok()?;
            let changed = bytes != m.bytes;
            let passes = match filter {
                AobFilter::Changed => changed,
                AobFilter::Unchanged => !changed,
            };
            passes.then_some(AobMatch {
                address: m.address,
                bytes,
                // What this scan compared against becomes the previous
                // value, mirroring `next_scan`.
                previous: m.bytes.clone(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_spaced_and_unspaced_hex() {
        assert_eq!(parse_hex_pattern("3F A2 01"), Ok(vec![0x3F, 0xA2, 0x01]));
        assert_eq!(parse_hex_pattern("3FA201"), Ok(vec![0x3F, 0xA2, 0x01]));
    }

    #[test]
    fn parses_mixed_case() {
        assert_eq!(parse_hex_pattern("3f a2"), Ok(vec![0x3F, 0xA2]));
    }

    #[test]
    fn rejects_odd_digit_count() {
        assert!(parse_hex_pattern("3FA").is_err());
    }

    #[test]
    fn rejects_non_hex_characters() {
        assert!(parse_hex_pattern("3G").is_err());
    }

    #[test]
    fn rejects_empty_or_whitespace_only() {
        assert!(parse_hex_pattern("").is_err());
        assert!(parse_hex_pattern("   ").is_err());
    }

    #[test]
    fn format_pattern_round_trips_through_parse() {
        let bytes = vec![0x3F, 0xA2, 0x00, 0xFF];
        let formatted = format_pattern(&bytes);
        assert_eq!(parse_hex_pattern(&formatted), Ok(bytes));
    }

    #[test]
    fn finds_a_pattern_sitting_exactly_at_a_chunk_boundary() {
        let pattern = [0xDE, 0xAD, 0xBE, 0xEF];
        let mut bytes = vec![0u8; 16];
        bytes[8..12].copy_from_slice(&pattern);

        let matches = scan_bytes_for_pattern(0, &bytes, &pattern);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].address, 8);
        assert_eq!(matches[0].bytes, pattern);
    }

    #[test]
    fn finds_unaligned_matches() {
        // Unlike numeric scanning, a pattern starting at a non-multiple
        // offset must still be found - AOB has no alignment requirement.
        let pattern = [0xAB, 0xCD, 0xEF];
        let mut bytes = vec![0u8; 10];
        bytes[3..6].copy_from_slice(&pattern); // offset 3, not aligned to 3

        let matches = scan_bytes_for_pattern(0, &bytes, &pattern);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].address, 3);
    }

    #[test]
    fn finds_overlapping_pattern_occurrences() {
        // "AAA" contains two overlapping occurrences of "AA".
        let pattern = [0xAA, 0xAA];
        let bytes = [0xAA, 0xAA, 0xAA];
        let matches = scan_bytes_for_pattern(0, &bytes, &pattern);
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].address, 0);
        assert_eq!(matches[1].address, 1);
    }
}
