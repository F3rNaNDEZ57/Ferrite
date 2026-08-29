//! Single-level pointer dereference: read a pointer-sized value at an
//! address, add an offset. The one pointer-chain shape v1 supports (see the
//! vault's `v1-scope.md`) — matches the single-hop case of Cheat Engine's
//! own multi-level algorithm (`TMemoryRecord.GetRealAddress`, verified by
//! reading CE's own source: for N offsets it dereferences-then-adds N
//! times, walking the offset list from last to first; for N=1 that's
//! exactly one dereference then one add, with no further dereference).

use crate::session::{MemoryError, ProcessSession};

/// Pointer width in the target process. v1 targets 64-bit processes only
/// (see the vault's `v1-scope.md`), so this is always 8 bytes.
const POINTER_SIZE: usize = 8;

/// Reads the pointer-sized value at `base` and adds `offset` to it — a
/// single dereference-then-add hop.
pub fn resolve_pointer(
    session: &ProcessSession,
    base: usize,
    offset: usize,
) -> Result<usize, MemoryError> {
    let bytes = session.read_bytes(base, POINTER_SIZE)?;
    let pointer = u64::from_le_bytes(
        bytes
            .try_into()
            .expect("read_bytes(_, POINTER_SIZE) returns exactly POINTER_SIZE bytes"),
    ) as usize;
    Ok(pointer.wrapping_add(offset))
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
