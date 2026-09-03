//! Text ↔ bytes for string-typed entries: the only genuinely new code a
//! string scan needs, since the scan itself is a byte-pattern search handed
//! straight to [`crate::aob`] (see the vault's `v0.2-plan.md`).
//!
//! A string entry's buffer length is fixed when the entry is created or
//! imported and never re-derived from memory afterwards. `zero_terminated`
//! therefore affects *display only* — it truncates the decoded text at the
//! first NUL found inside that fixed buffer, it never resizes the buffer or
//! turns a refresh into a byte-by-byte hunt for a terminator.

use serde::{Deserialize, Serialize};

/// The two string encodings Ferrite supports, matching the two Cheat Engine
/// itself exposes (`MemoryRecordUnit.pas`'s `stringData.unicode` flag).
/// Nothing else — a `.CT` entry carrying `<CodePage>1</CodePage>` is a third
/// thing (Windows-codepage text) and is reported as unsupported rather than
/// guessed at as Latin-1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextEncoding {
    /// One byte per character, byte value == code point — Cheat Engine's
    /// plain `String`. "Latin-1" rather than "ASCII" is the honest name:
    /// bytes 0x80–0xFF decode to U+0080–U+00FF here rather than being
    /// rejected or replaced, which is what makes encode/decode a true round
    /// trip.
    Latin1,
    /// UTF-16, little-endian — Cheat Engine's `Unicode String` (equivalently
    /// `String` with `<Unicode>1</Unicode>`). Little-endian because the
    /// target is x86-64 Windows, where that's what a `WideString` is.
    Utf16Le,
}

impl TextEncoding {
    /// Bytes per character in this encoding. A `.CT` `<Length>` is a
    /// *character* count, so this is the multiplier that turns it into a
    /// buffer size — verified against Cheat Engine's own
    /// `TMemoryRecord.getByteSize`, which returns `stringData.length`
    /// doubled when `unicode` is set (`MemoryRecordUnit.pas`).
    pub fn bytes_per_char(self) -> usize {
        match self {
            Self::Latin1 => 1,
            Self::Utf16Le => 2,
        }
    }
}

/// Encodes `text` for a scan or a write. Fails only for [`TextEncoding::Latin1`]
/// text containing a character above U+00FF, which has no single-byte
/// representation — reported rather than silently replaced, so a search that
/// could never match anything is never quietly issued.
pub fn encode_text(text: &str, encoding: TextEncoding) -> Result<Vec<u8>, String> {
    match encoding {
        TextEncoding::Latin1 => text
            .chars()
            .map(|c| {
                u8::try_from(c as u32).map_err(|_| {
                    format!("{c:?} can't be encoded as a single byte - use Unicode String instead")
                })
            })
            .collect(),
        TextEncoding::Utf16Le => Ok(text.encode_utf16().flat_map(u16::to_le_bytes).collect()),
    }
}

/// Decodes a fixed-length buffer read from the target process for display.
///
/// `zero_terminated` truncates at the first NUL *character* — for
/// [`TextEncoding::Utf16Le`] that's the first zero 16-bit unit, not the first
/// zero byte, since every ASCII character in UTF-16LE already contains one
/// (`"AB"` is `41 00 42 00`). Never fails: this decodes whatever bytes are
/// live in another process's memory, which are under no obligation to be
/// well-formed text, so unpaired surrogates become U+FFFD rather than an
/// error a display path would have to invent something to show for.
pub fn decode_text(bytes: &[u8], encoding: TextEncoding, zero_terminated: bool) -> String {
    match encoding {
        TextEncoding::Latin1 => {
            let bytes = match zero_terminated {
                true => &bytes[..bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len())],
                false => bytes,
            };
            bytes.iter().map(|&b| b as char).collect()
        }
        TextEncoding::Utf16Le => {
            // A trailing odd byte can't form a unit and is dropped: a
            // buffer whose length isn't a whole number of characters is
            // possible (a hand-typed length, a truncated read) and isn't
            // worth failing a display path over.
            let (pairs, _odd) = bytes.as_chunks::<2>();
            let mut units: Vec<u16> = pairs.iter().map(|&p| u16::from_le_bytes(p)).collect();
            if zero_terminated && let Some(end) = units.iter().position(|&u| u == 0) {
                units.truncate(end);
            }
            String::from_utf16_lossy(&units)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latin1_round_trips_bytes_above_ascii() {
        // The asymmetry this guards: `String::from_utf8_lossy` would decode
        // 0xE9 to U+FFFD and break the round trip, since 0xE9 alone isn't
        // valid UTF-8. Latin-1 is a byte-value == code-point mapping, not
        // a UTF-8 one.
        let encoded = encode_text("caf\u{e9}", TextEncoding::Latin1).expect("Latin-1 encodable");
        assert_eq!(encoded, vec![b'c', b'a', b'f', 0xE9]);
        assert_eq!(
            decode_text(&encoded, TextEncoding::Latin1, false),
            "caf\u{e9}"
        );
    }

    #[test]
    fn latin1_rejects_a_character_it_cant_represent() {
        let err = encode_text("日本", TextEncoding::Latin1).expect_err("not Latin-1 encodable");
        assert!(
            err.contains("Unicode String"),
            "the error should point at the type that can represent it, got: {err}"
        );
    }

    #[test]
    fn utf16le_round_trips() {
        let encoded = encode_text("Hi", TextEncoding::Utf16Le).expect("UTF-16 never fails");
        assert_eq!(encoded, vec![b'H', 0, b'i', 0]);
        assert_eq!(decode_text(&encoded, TextEncoding::Utf16Le, false), "Hi");
    }

    #[test]
    fn utf16le_nul_truncation_is_unit_aligned_not_byte_aligned() {
        // "AB" is 41 00 42 00 - a byte-wise search for the first zero would
        // truncate this to "A".
        let bytes = [0x41, 0x00, 0x42, 0x00];
        assert_eq!(decode_text(&bytes, TextEncoding::Utf16Le, true), "AB");

        // A real UTF-16 terminator is a whole zero *unit*.
        let bytes = [0x41, 0x00, 0x00, 0x00, 0x42, 0x00];
        assert_eq!(decode_text(&bytes, TextEncoding::Utf16Le, true), "A");
    }

    #[test]
    fn zero_terminate_truncates_for_display_only_within_the_fixed_buffer() {
        // The shape a `<Length>8</Length>` entry actually has in memory:
        // text, then NUL padding to the declared length.
        let buffer = b"Bob\0\0\0\0\0";
        assert_eq!(decode_text(buffer, TextEncoding::Latin1, true), "Bob");
        // Unset, the padding decodes as the NUL characters it is - the
        // buffer is never re-sized either way.
        assert_eq!(
            decode_text(buffer, TextEncoding::Latin1, false),
            "Bob\0\0\0\0\0"
        );
    }

    #[test]
    fn an_odd_trailing_byte_is_dropped_rather_than_failing() {
        let bytes = [0x41, 0x00, 0x42];
        assert_eq!(decode_text(&bytes, TextEncoding::Utf16Le, false), "A");
    }

    #[test]
    fn a_length_in_characters_becomes_a_buffer_size_in_bytes() {
        assert_eq!(TextEncoding::Latin1.bytes_per_char(), 1);
        assert_eq!(TextEncoding::Utf16Le.bytes_per_char(), 2);
    }
}
