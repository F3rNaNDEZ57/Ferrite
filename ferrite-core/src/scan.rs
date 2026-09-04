//! The scan engine: an exact-value first scan across a process's writable
//! memory regions, and next-scan filters that re-check previously matched
//! addresses.

use crate::regions::MemoryRegion;
use crate::scan_value::{ScanFilter, ScanValue, bytes_match_exact, passes_filter};
use crate::session::ProcessSession;

/// "2 decimal places" — matches typical in-game display precision (health,
/// currency) — is the default rounding used for exact-value float matches.
pub const DEFAULT_FLOAT_DECIMALS: u32 = 2;

/// How large a chunk of memory to read at once. Regions can be hundreds of
/// MB (a committed heap region, for instance), so a scan reads in bounded
/// chunks rather than pulling a whole region into memory at once.
pub const DEFAULT_CHUNK_SIZE: usize = 1024 * 1024; // 1 MiB

/// Stop a first scan once this many matches are found, rather than
/// potentially returning millions of results into a `Vec`/GUI table that
/// would hang the first demo. A correctness floor for M1, not later polish
/// — see the vault's `v1-plan.md`.
pub const DEFAULT_MAX_RESULTS: usize = 50_000;

/// Tunable parameters for a scan. `Default` gives sane values for real use;
/// tests override individual fields (e.g. a tiny `chunk_size`) to exercise
/// edge cases a real process's memory layout can't reliably reproduce on
/// demand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanOptions {
    pub float_decimals: u32,
    pub chunk_size: usize,
    pub max_results: usize,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            float_decimals: DEFAULT_FLOAT_DECIMALS,
            chunk_size: DEFAULT_CHUNK_SIZE,
            max_results: DEFAULT_MAX_RESULTS,
        }
    }
}

/// One matched address from a scan, with the value found there and the one
/// it held before the most recent re-read.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScanMatch {
    pub address: usize,
    pub value: ScanValue,
    /// What `value` was before the last [`next_scan`]. Equal to `value` on a
    /// first scan, where there is no earlier reading to show — not an
    /// `Option`, because "no previous value yet" and "unchanged since the
    /// last scan" render identically and an `Option` would only push that
    /// decision out to every call site.
    ///
    /// Deliberately *not* updated by the GUI's live refresh: this is the
    /// value at the previous **scan**, which is what a rescan filtered on,
    /// and it would stop meaning that if a background tick moved it.
    pub previous: ScanValue,
}

/// Result of a first scan. `capped` is true if `max_results` was hit before
/// the scan finished walking every region — the GUI should tell the user to
/// narrow their search, not present this as a complete result set.
#[derive(Debug, Clone, PartialEq)]
pub struct FirstScanResult {
    pub matches: Vec<ScanMatch>,
    pub capped: bool,
}

/// Performs an exact-value first scan: walks every writable region and,
/// within each, every address aligned to `target`'s type size, checking for
/// a match against `target`.
pub fn first_scan_exact(
    session: &ProcessSession,
    target: ScanValue,
    options: ScanOptions,
) -> FirstScanResult {
    let mut matches = Vec::new();

    for region in session.writable_regions() {
        let remaining_budget = options.max_results - matches.len();
        if remaining_budget == 0 {
            break;
        }
        matches.extend(scan_region_exact(
            session,
            region,
            target,
            options,
            remaining_budget,
        ));
    }

    let capped = matches.len() >= options.max_results;
    FirstScanResult { matches, capped }
}

/// Re-checks previously matched addresses against `filter`, returning the
/// surviving matches with their values updated to what's there *now*.
///
/// Deliberately dumb: no chunking, no region walk — `matches` is a list of
/// discrete addresses to individually re-read, not a memory range to scan,
/// so a straight loop of small reads is both correct and simpler than
/// reusing the first-scan machinery here.
///
/// Updating the stored value on every surviving match (rather than keeping
/// the value from the *previous* scan) matters: without it, a second
/// `Changed` scan in a row would compare against an already-stale baseline
/// and report changes that already happened on the first scan. A match
/// whose address can no longer be read (the page was freed, or the process
/// is exiting) is dropped, not retained with a stale value — a retained but
/// unreadable entry would be a phantom result the GUI has nothing to show.
pub fn next_scan(
    session: &ProcessSession,
    matches: &[ScanMatch],
    filter: ScanFilter,
) -> Vec<ScanMatch> {
    matches
        .iter()
        .filter_map(|m| {
            let bytes = session.read_bytes(m.address, m.value.size()).ok()?;
            let new_value = m.value.from_le_bytes_like(&bytes);
            passes_filter(m.value, new_value, filter).then_some(ScanMatch {
                address: m.address,
                value: new_value,
                // The value this scan filtered against becomes the previous
                // one, so the column always shows what the comparison was
                // actually made with.
                previous: m.value,
            })
        })
        .collect()
}

/// Scans a single region for an exact-value match against `target`,
/// returning at most `max_matches` results.
///
/// A standalone building block, not just an implementation detail of
/// [`first_scan_exact`]: it's also what integration tests use to exercise
/// real `ReadProcessMemory` calls against a tightly-bounded region (e.g. a
/// synthetic region wrapping one known address), without having to scan an
/// entire process's real memory to do it.
pub fn scan_region_exact(
    session: &ProcessSession,
    region: MemoryRegion,
    target: ScanValue,
    options: ScanOptions,
    max_matches: usize,
) -> Vec<ScanMatch> {
    let value_size = target.size();
    let overlap = value_size.saturating_sub(1);
    let mut matches = Vec::new();
    let mut offset = 0usize;

    while offset < region.size {
        let remaining = region.size - offset;
        // Read `chunk_size` bytes plus `overlap` extra so a value
        // straddling where this chunk ends is still fully covered by this
        // read, and gets found here rather than silently missed: without
        // the overlap, a value spanning the boundary would need bytes from
        // both this chunk and the next, and a naive non-overlapping chunk
        // loop would skip it with no error at all — just an absent result.
        let read_len = remaining.min(options.chunk_size + overlap);
        let chunk_address = region.base_address + offset;

        // Tolerate a chunk read failing outright — a page can become
        // inaccessible between VirtualQueryEx and this read, or the process
        // can be exiting. Skip this chunk and keep scanning the rest of the
        // region rather than aborting the whole scan over one bad chunk.
        if let Ok(bytes) = session.read_bytes(chunk_address, read_len) {
            for m in scan_bytes_for_value(chunk_address, &bytes, target, options.float_decimals) {
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

/// Scans `bytes` (read from `base_address` in the target process) for every
/// aligned occurrence of `target`.
///
/// Candidate offsets are aligned to the *absolute* address
/// (`base_address + local_offset`), not the buffer-relative offset: once
/// chunks overlap by a non-multiple-of-`value_size` amount, chunk starts
/// shift by non-multiples of the type size, and offset-relative alignment
/// would silently drift which positions get checked.
///
/// Pure and process-free — this is what the chunk-overlap behavior above is
/// actually tested against, with a `bytes` buffer and `base_address` chosen
/// to force a value across a chunk boundary.
fn scan_bytes_for_value(
    base_address: usize,
    bytes: &[u8],
    target: ScanValue,
    float_decimals: u32,
) -> Vec<ScanMatch> {
    let value_size = target.size();
    let mut matches = Vec::new();
    if bytes.len() < value_size {
        return matches;
    }

    let mut candidate = base_address.next_multiple_of(value_size);
    while candidate + value_size <= base_address + bytes.len() {
        let local_offset = candidate - base_address;
        let slice = &bytes[local_offset..local_offset + value_size];
        if bytes_match_exact(slice, target, float_decimals) {
            let found = target.from_le_bytes_like(slice);
            matches.push(ScanMatch {
                address: candidate,
                value: found,
                // Nothing has changed yet on a first scan.
                previous: found,
            });
        }
        candidate += value_size;
    }
    matches
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_a_value_sitting_exactly_at_a_chunk_boundary() {
        // Suppose chunks were (wrongly) read without overlap, split at
        // offset 8: a value occupying bytes [8..12) needs bytes from both
        // the "before 8" chunk and the "from 8 on" chunk. A naive
        // non-overlapping split would give neither chunk the full 4 bytes,
        // and the value would be missed with no error at all. The overlap
        // logic in first_scan_exact reads past the nominal chunk end
        // specifically so this buffer — which already includes the overlap
        // — contains the value whole, and scan_bytes_for_value finds it.
        let target = ScanValue::I32(0x1234_5678);
        let mut bytes = vec![0u8; 16];
        bytes[8..12].copy_from_slice(&0x1234_5678i32.to_le_bytes());
        let base_address = 0usize;

        let matches = scan_bytes_for_value(base_address, &bytes, target, 2);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].address, 8);
        assert_eq!(matches[0].value, target);
    }

    #[test]
    fn base_address_offset_shifts_which_positions_are_aligned() {
        // Same buffer, but as if this chunk started at an odd address:
        // alignment must be computed from the absolute address, not the
        // buffer-relative offset, or this value (at absolute address 13,
        // not a multiple of 4) would be wrongly treated as a candidate.
        let target = ScanValue::I32(42);
        let mut bytes = vec![0u8; 16];
        bytes[1..5].copy_from_slice(&42i32.to_le_bytes()); // absolute address 13
        let base_address = 12usize; // buffer[1] is absolute address 13

        let matches = scan_bytes_for_value(base_address, &bytes, target, 2);
        assert!(
            matches.is_empty(),
            "address 13 isn't 4-byte aligned and must not match: {matches:?}"
        );

        // Place it 4-byte aligned relative to the absolute address instead
        // (absolute address 16, buffer offset 4) and confirm it IS found.
        let mut bytes = vec![0u8; 16];
        bytes[4..8].copy_from_slice(&42i32.to_le_bytes()); // absolute address 16
        let matches = scan_bytes_for_value(base_address, &bytes, target, 2);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].address, 16);
    }
}
