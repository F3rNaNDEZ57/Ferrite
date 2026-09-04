//! Multi-level pointer resolution: walk a chain of dereference-then-add
//! hops to a final address.
//!
//! The algorithm is Cheat Engine's own `TMemoryRecord.GetRealAddress`
//! (`MemoryRecordUnit.pas`), read from the source rather than inferred.
//! Two details of it decide whether a chain lands on the right address at
//! all, and both are verified rather than assumed:
//!
//! - **Offsets are stored in document order.** CE's XML reader fills
//!   `fpointeroffsets[j]` walking `<Offset>` elements front to back, and
//!   its writer emits them back in that same index order — neither end
//!   reverses.
//! - **The walk runs last to first.** `for i := offsetCount-1 downto 0`,
//!   each iteration dereferencing the running address and adding
//!   `offsets[i]`. The address after the index-0 iteration *is* the answer:
//!   there are exactly N dereferences for N offsets, with no trailing one.
//!
//! Getting either backwards resolves to a wrong address without failing, so
//! both are covered by tests against a real two-hop chain in
//! `ferrite-victim`, not just by arithmetic.

use crate::session::{MemoryError, ProcessSession};

/// Pointer width in the target process. Ferrite targets 64-bit processes
/// only (see the vault's `v0.1-scope.md`), so this is always 8 bytes.
const POINTER_SIZE: usize = 8;

/// The longest chain accepted from a `.CT` file or a manual-add form. Real
/// tables top out around six or seven offsets; the cap keeps a malformed or
/// adversarial table from driving an arbitrarily long sequence of reads on
/// every refresh tick. Enforced where chains are *accepted* rather than
/// inside the resolver, so that resolution itself stays a pure walk of
/// whatever it's handed.
pub const MAX_POINTER_CHAIN_DEPTH: usize = 16;

/// Walks `offsets` from last to first, dereferencing and adding at each
/// hop, and returns the final address.
///
/// An empty slice returns `base` unchanged **without reading memory at
/// all** — that's the direct-address case, which must not turn into a
/// single stray dereference.
pub fn resolve_pointer_chain(
    session: &ProcessSession,
    base: usize,
    offsets: &[usize],
) -> Result<usize, MemoryError> {
    let mut address = base;
    for &offset in offsets.iter().rev() {
        let bytes = session.read_bytes(address, POINTER_SIZE)?;
        let pointer = u64::from_le_bytes(
            bytes
                .try_into()
                .expect("read_bytes(_, POINTER_SIZE) returns exactly POINTER_SIZE bytes"),
        ) as usize;
        address = pointer.wrapping_add(offset);
    }
    Ok(address)
}

/// Reads the pointer-sized value at `base` and adds `offset` to it — a
/// single dereference-then-add hop.
///
/// Deliberately a thin wrapper rather than a second implementation: it
/// keeps the single-hop integration test that has covered this since M3
/// working as the chain resolver's base-case proof.
pub fn resolve_pointer(
    session: &ProcessSession,
    base: usize,
    offset: usize,
) -> Result<usize, MemoryError> {
    resolve_pointer_chain(session, base, &[offset])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_a_pointer_in_our_own_process() {
        // A real pointer: the address of a local `u32`, stored in a local
        // `usize`, read back through `resolve_pointer` exactly as it would
        // be for an attached target process.
        let target: u32 = 0xDEAD_BEEF;
        let target_address = &raw const target as usize;
        let pointer_holder: usize = target_address;

        let session =
            ProcessSession::attach(std::process::id()).expect("attaching to our own process");
        let pointer_holder_address = &raw const pointer_holder as usize;

        let resolved = resolve_pointer(&session, pointer_holder_address, 0)
            .expect("resolving a real pointer in our own process");
        assert_eq!(resolved, target_address);
    }

    #[test]
    fn an_empty_chain_returns_the_base_without_dereferencing_it() {
        // The direct-address case. `base` here is deliberately an address
        // no read could succeed at, so a stray dereference would surface as
        // an error rather than passing by luck.
        let session =
            ProcessSession::attach(std::process::id()).expect("attaching to our own process");
        assert_eq!(resolve_pointer_chain(&session, 0x1, &[]), Ok(0x1));
    }

    #[test]
    fn a_two_hop_chain_walks_its_offsets_last_to_first() {
        // A real two-level chain built in our own process: `outer` points at
        // `inner`, which points at `target`. Offsets are in document order,
        // so [0x0, 0x0] means "dereference outer, add 0; dereference that,
        // add 0" - two dereferences for two offsets, ending on `target`.
        let target: u32 = 0xDEAD_BEEF;
        let inner: usize = &raw const target as usize;
        let outer: usize = &raw const inner as usize;

        let session =
            ProcessSession::attach(std::process::id()).expect("attaching to our own process");

        let resolved = resolve_pointer_chain(&session, &raw const outer as usize, &[0, 0])
            .expect("resolving a real two-hop chain");
        assert_eq!(resolved, &raw const target as usize);

        // One offset stops a hop earlier - the proof that N offsets means
        // exactly N dereferences, with no trailing one.
        let one_hop = resolve_pointer_chain(&session, &raw const outer as usize, &[0])
            .expect("resolving one hop of the same chain");
        assert_eq!(one_hop, &raw const inner as usize);
    }

    #[test]
    fn the_last_offset_is_applied_at_the_first_hop() {
        // Distinguishes last-to-first from first-to-last with a chain whose
        // two offsets differ, so the two walk orders land on different
        // addresses rather than coincidentally agreeing.
        //
        //   outer -> mid -> leaf[0]
        //
        // Offsets in document order are [0x10, 0x00]: the *last* one (0x00)
        // is applied at the first hop, landing on `mid` exactly; the
        // *first* one (0x10) is applied at the second, stepping two u64s
        // into `leaf`. Walked the other way round, the first hop would add
        // 0x10 to `mid`'s address and the chain would end up somewhere
        // unrelated.
        let leaf: [u64; 4] = [0, 0, 0xDEAD_BEEF, 0];
        let mid: usize = &raw const leaf[0] as usize;
        let outer: usize = &raw const mid as usize;

        let session =
            ProcessSession::attach(std::process::id()).expect("attaching to our own process");

        let resolved = resolve_pointer_chain(&session, &raw const outer as usize, &[0x10, 0x00])
            .expect("resolving the chain");
        assert_eq!(resolved, &raw const leaf[2] as usize);

        // And the value really is there, read through the resolved address.
        let bytes = session.read_bytes(resolved, 8).expect("reading leaf[2]");
        assert_eq!(u64::from_le_bytes(bytes.try_into().unwrap()), 0xDEAD_BEEF);
    }

    #[test]
    fn adds_the_offset_after_dereferencing() {
        // `buffer`'s own bytes, read as a little-endian u64, are the
        // "pointer value" resolve_pointer should dereference to.
        let buffer: [u8; 8] = 0xAAAA_AAAA_AAAA_AAAAu64.to_le_bytes();
        let base = &raw const buffer as usize;

        let session =
            ProcessSession::attach(std::process::id()).expect("attaching to our own process");

        let resolved =
            resolve_pointer(&session, base, 0x10).expect("resolving a pointer in our own process");
        assert_eq!(
            resolved,
            0xAAAA_AAAA_AAAA_AAAAu64.wrapping_add(0x10) as usize
        );
    }
}
