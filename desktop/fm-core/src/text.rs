//! Text out of a file's bytes, which are not always UTF-8.
//!
//! `String::from_utf8_lossy` turns every byte it does not understand into
//! U+FFFD, so a name written on a Windows machine arrives as a row of
//! replacement characters: the report then says less about the file than the
//! file does, which is the one thing this tool must not do.
//!
//! What the formats actually promise:
//!
//! - PDF strings are UTF-16 when they start with a byte-order mark and
//!   PDFDocEncoding otherwise, which agrees with Latin-1 over the accented
//!   range.
//! - PNG `tEXt` and `zTXt` are Latin-1 by specification; `iTXt` is UTF-8.
//! - IPTC is ISO-8859-1 unless 1:90 says otherwise.
//! - EXIF `ASCII` fields are ASCII by specification and routinely are not.
//! - An MP4 `data` box states its own encoding, which [`super::format::mp4`]
//!   reads before calling here.
//!
//! So: a byte-order mark wins, then valid UTF-8, then a single-byte
//! codepage. Nothing here ever produces U+FFFD from a non-empty input.

/// How a value was read, so that a report can say when it was a fallback
/// rather than something the file stated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    Utf8,
    /// From a byte-order mark.
    Utf16,
    /// Read byte by byte, because the bytes are not valid UTF-8.
    SingleByte,
}

impl Encoding {
    /// What to add to a finding when the encoding was not the file's own
    /// claim. A wrong accent is worth explaining; a right one is not worth a
    /// sentence.
    pub fn caveat(self) -> Option<&'static str> {
        match self {
            Encoding::SingleByte => Some(
                "This field is not UTF-8, so it was read a byte at a time. Accents written \
                 in a Central European codepage can come out as the wrong letter here, \
                 while the bytes in the file are untouched either way.",
            ),
            Encoding::Utf8 | Encoding::Utf16 => None,
        }
    }
}

/// Decode, and say how.
pub fn decode(bytes: &[u8]) -> (String, Encoding) {
    if let Some(text) = utf16(bytes) {
        return (text, Encoding::Utf16);
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => (s.to_string(), Encoding::Utf8),
        // Every byte is a codepoint. Over 0xA0 this is Latin-1, which is also
        // what PDF and PNG specify and what Windows-1252 agrees with; below it
        // the C1 range is not text at all, so those bytes are dropped rather
        // than turned into invisible control characters.
        Err(_) => (
            bytes
                .iter()
                .filter(|b| !(0x80..=0x9f).contains(*b))
                .map(|b| *b as char)
                .collect(),
            Encoding::SingleByte,
        ),
    }
}

/// Decode and throw the encoding away, for the many places that only want the
/// text.
pub fn text(bytes: &[u8]) -> String {
    decode(bytes).0
}

/// A value as a report wants it: decoded, with the padding a fixed-width field
/// leaves behind taken off.
pub fn trimmed(bytes: &[u8]) -> (String, Encoding) {
    let (s, enc) = decode(bytes);
    (s.trim_end_matches('\0').trim().to_string(), enc)
}

/// UTF-16 big-endian with no byte-order mark, which is what an MP4 `data` box
/// with well-known type 2 holds. The type says so, so there is nothing to
/// guess and nothing to caveat.
pub fn utf16be(bytes: &[u8]) -> String {
    String::from_utf16_lossy(&units(bytes, true))
}

/// Pairs of bytes as code units. Written with indices rather than
/// `chunks_exact` because the tidier `as_chunks` wants a newer compiler than
/// the `rust-version` in Cargo.toml promises.
fn units(bytes: &[u8], big: bool) -> Vec<u16> {
    (0..bytes.len() / 2)
        .map(|i| {
            let (a, b) = (bytes[2 * i], bytes[2 * i + 1]);
            if big {
                u16::from_be_bytes([a, b])
            } else {
                u16::from_le_bytes([a, b])
            }
        })
        .collect()
}

fn utf16(bytes: &[u8]) -> Option<String> {
    let (rest, big) = match bytes {
        [0xfe, 0xff, rest @ ..] => (rest, true),
        [0xff, 0xfe, rest @ ..] => (rest, false),
        _ => return None,
    };
    // An unpaired surrogate is a broken string, not a reason to show nothing,
    // so the replacement there is deliberate and local.
    Some(String::from_utf16_lossy(&units(rest, big)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_is_read_as_itself() {
        let (s, enc) = decode("Jürgen Müller, Plzeň".as_bytes());
        assert_eq!(s, "Jürgen Müller, Plzeň");
        assert_eq!(enc, Encoding::Utf8);
        assert!(enc.caveat().is_none());
    }

    /// What a Windows tool writes into an EXIF field, and what used to arrive
    /// as a row of replacement characters.
    #[test]
    fn a_windows_codepage_keeps_its_letters_rather_than_becoming_question_marks() {
        let cp1252 = b"J\xfcrgen M\xfcller";
        let (s, enc) = decode(cp1252);
        assert_eq!(s, "Jürgen Müller");
        assert_eq!(enc, Encoding::SingleByte);
        assert!(enc.caveat().is_some(), "a guessed encoding has to say so");
        assert!(!s.contains('\u{fffd}'));
    }

    /// The accents that differ between codepages are the ones to be honest
    /// about: this is Windows-1250 for "Plzeň", and Latin-1 cannot know that.
    #[test]
    fn a_central_european_codepage_is_readable_even_where_it_is_wrong() {
        let (s, enc) = decode(b"Plze\xf2");
        assert_eq!(s, "Plzeò", "the bytes say 0xF2, which is this letter in Latin-1");
        assert_eq!(enc, Encoding::SingleByte);
        assert!(!s.contains('\u{fffd}'));
    }

    /// The C1 range is where Windows-1250 keeps half the Czech alphabet and
    /// Latin-1 keeps control codes. Showing a control code is showing nothing,
    /// so those bytes go and the rest of the value survives.
    #[test]
    fn control_bytes_are_dropped_rather_than_drawn() {
        let (s, _) = decode(b"Krej\x9f\xedk");
        assert_eq!(s, "Krejík");
        assert!(!s.chars().any(|c| c.is_control()));
    }

    #[test]
    fn a_byte_order_mark_is_obeyed_both_ways() {
        let be = [0xfe, 0xff, 0x00, 0x4a, 0x00, 0xfc, 0x00, 0x72];
        let le = [0xff, 0xfe, 0x4a, 0x00, 0xfc, 0x00, 0x72, 0x00];
        assert_eq!(decode(&be), ("Jür".to_string(), Encoding::Utf16));
        assert_eq!(decode(&le), ("Jür".to_string(), Encoding::Utf16));
    }

    #[test]
    fn padding_comes_off_a_fixed_width_field() {
        assert_eq!(trimmed(b"Apple\0\0\0").0, "Apple");
        assert_eq!(trimmed(b"  Apple  ").0, "Apple");
    }

    #[test]
    fn nothing_in_is_nothing_out() {
        assert_eq!(decode(b""), (String::new(), Encoding::Utf8));
    }
}
