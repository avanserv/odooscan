//! The text of a data file. Odoo hands the bytes of an XML file to `etree.parse` on every series,
//! so libxml2 picks the encoding, not Python: 9.0 `openerp/tools/convert.py:838,915`, 10.0
//! `odoo/tools/convert.py:835,899`, 11.0 `:784,836`, 18.0 `:659,715`, 20.0 `:712,792`.
//!
//! A CSV goes through `ustr` cell by cell below 11.0, UTF-8 first, then Latin-1, which cannot fail:
//! 8.0 `openerp/tools/convert.py:951`, 9.0 `:899` with `openerp/loglevels.py:28`, 10.0
//! `odoo/tools/convert.py:882` with `odoo/loglevels.py:28`. From 11.0 it is strict UTF-8: 11.0
//! `odoo/tools/convert.py:810` with `odoo/tools/pycompat.py:46-48,89-94`, 17.0 `:700`, 18.0 `:683`,
//! 20.0 `:741`.

use std::borrow::Cow;
use std::sync::LazyLock;

use regex::bytes::Regex;

use crate::series::Series;

/// The first series that reads a data CSV as strict UTF-8 (`11.0`).
const CSV_UTF8: Series = Series { major: 11, minor: 0 };

/// A declaration opens the file or is none, so a BOM ahead of it hides it: libxml2 lets a BOM beat
/// the declaration (probed on lxml 5.2.1).
static DECLARED_ENCODING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?-u)^<\?xml\s+(?:version\s*=\s*(?:"[^"]*"|'[^']*')\s+)?encoding\s*=\s*["']([^"']*)["']"#).unwrap()
});

fn latin1(byte: u8) -> char {
    char::from(byte)
}

/// ISO-8859-15 is ISO-8859-1 but for these eight bytes.
fn latin9(byte: u8) -> char {
    match byte {
        0xA4 => '\u{20AC}',
        0xA6 => '\u{0160}',
        0xA8 => '\u{0161}',
        0xB4 => '\u{017D}',
        0xB8 => '\u{017E}',
        0xBC => '\u{0152}',
        0xBD => '\u{0153}',
        0xBE => '\u{0178}',
        _ => char::from(byte),
    }
}

fn single_byte_map(label: &[u8]) -> Option<fn(u8) -> char> {
    match label.to_ascii_uppercase().as_slice() {
        b"ISO-8859-1" | b"ISO_8859-1" | b"ISO8859-1" | b"ISO88591" | b"ISO-LATIN-1" | b"ISO-IR-100" | b"LATIN1"
        | b"L1" | b"CP819" | b"IBM819" | b"CSISOLATIN1" => Some(latin1),
        b"ISO-8859-15" | b"ISO_8859-15" | b"ISO8859-15" | b"ISO885915" | b"ISO-IR-203" | b"LATIN-9" | b"LATIN9" => {
            Some(latin9)
        }
        _ => None,
    }
}

/// The text of `bytes` in the encoding they declare, or none when they do not decode. A label other
/// than Latin-1 or Latin-9 reads as UTF-8, which leaves a file in another encoding unread.
pub fn xml_text(bytes: &[u8]) -> Option<Cow<'_, str>> {
    let declared = DECLARED_ENCODING.captures(bytes);
    match declared.and_then(|c| single_byte_map(&c[1])) {
        Some(map) => Some(Cow::Owned(bytes.iter().copied().map(map).collect())),
        None => std::str::from_utf8(bytes).ok().map(Cow::Borrowed),
    }
}

/// The text of a data CSV as `series` reads it, or none when it does not decode. An unknown series,
/// which a `10.saas~N` version parses to, stays strict: an unread file keeps the rules silent.
pub fn csv_text(bytes: &[u8], series: Option<Series>) -> Option<Cow<'_, str>> {
    if let Ok(text) = std::str::from_utf8(bytes) {
        return Some(Cow::Borrowed(text));
    }
    let lenient = series.is_some_and(|s| s < CSV_UTF8);
    lenient.then(|| Cow::Owned(csv_cells(bytes).map(csv_cell).collect()))
}

/// The cells of a CSV, each with the separator that ends it, so that they join back into the file
/// with its line numbers.
fn csv_cells(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    let mut quoted = false;
    bytes.split_inclusive(move |&byte| {
        quoted ^= byte == b'"';
        !quoted && matches!(byte, b',' | b'\r' | b'\n')
    })
}

fn csv_cell(cell: &[u8]) -> Cow<'_, str> {
    match std::str::from_utf8(cell) {
        Ok(text) => Cow::Borrowed(text),
        Err(_) => Cow::Owned(cell.iter().copied().map(latin1).collect()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn series(text: &str) -> Option<Series> {
        Series::parse(text)
    }

    fn declared(label: &str, body: &[u8]) -> Vec<u8> {
        let mut bytes = format!("<?xml version=\"1.0\" encoding=\"{label}\"?>").into_bytes();
        bytes.extend_from_slice(body);
        bytes
    }

    #[test]
    fn reads_each_latin1_label() {
        for label in [
            "ISO-8859-1",
            "ISO_8859-1",
            "ISO8859-1",
            "ISO88591",
            "ISO-LATIN-1",
            "ISO-IR-100",
            "latin1",
            "l1",
            "CP819",
            "IBM819",
            "csISOLatin1",
        ] {
            let bytes = declared(label, b"\xE9");
            assert!(xml_text(&bytes).unwrap().ends_with('é'), "{label}");
        }
    }

    #[test]
    fn reads_each_latin9_label() {
        for label in [
            "ISO-8859-15",
            "ISO_8859-15",
            "ISO8859-15",
            "ISO885915",
            "ISO-IR-203",
            "latin-9",
            "latin9",
        ] {
            let bytes = declared(label, b"\xBD");
            assert!(xml_text(&bytes).unwrap().ends_with('œ'), "{label}");
        }
    }

    #[test]
    fn reads_a_label_in_any_case() {
        assert!(xml_text(&declared("Iso-8859-1", b"\xE9")).is_some());
        assert!(xml_text(&declared("LATIN1", b"\xE9")).is_some());
        assert!(xml_text(&declared("Latin-9", b"\xE9")).is_some());
    }

    #[test]
    fn reads_a_declaration_in_single_quotes() {
        let text = xml_text(b"<?xml version='1.0' encoding='ISO-8859-1'?>\xE9").unwrap();
        assert!(text.ends_with('é'));
    }

    #[test]
    fn reads_a_declaration_without_a_version() {
        assert!(xml_text(b"<?xml encoding=\"latin1\"?>\xE9").is_some());
    }

    #[test]
    fn a_bom_beats_a_latin1_declaration() {
        let mut bytes = b"\xEF\xBB\xBF".to_vec();
        bytes.extend(declared("ISO-8859-1", b"\xE9"));
        assert!(xml_text(&bytes).is_none());
        let mut valid = b"\xEF\xBB\xBF".to_vec();
        valid.extend(declared("ISO-8859-1", "é".as_bytes()));
        assert!(xml_text(&valid).unwrap().ends_with('é'));
    }

    #[test]
    fn undeclared_high_bytes_do_not_decode() {
        assert!(xml_text(b"<odoo>\xE9</odoo>").is_none());
        assert!(xml_text(b"<?xml version=\"1.0\"?><odoo>\xE9</odoo>").is_none());
    }

    #[test]
    fn a_declaration_past_the_first_byte_is_none() {
        assert!(xml_text(b"\n<?xml version=\"1.0\" encoding=\"latin1\"?>\xE9").is_none());
    }

    #[test]
    fn an_unknown_label_reads_as_utf8() {
        assert!(xml_text(&declared("windows-1252", b"ok")).is_some());
        assert!(xml_text(&declared("windows-1252", b"\xE9")).is_none());
        assert!(xml_text(&declared("UTF-8", "é".as_bytes())).is_some());
    }

    #[test]
    fn utf8_stays_borrowed() {
        assert!(matches!(xml_text("<odoo>é</odoo>".as_bytes()), Some(Cow::Borrowed(_))));
    }

    #[test]
    fn a4_differs_between_the_two() {
        assert!(xml_text(&declared("ISO-8859-1", b"\xA4")).unwrap().ends_with('¤'));
        assert!(xml_text(&declared("ISO-8859-15", b"\xA4")).unwrap().ends_with('€'));
    }

    #[test]
    fn latin9_maps_all_eight_bytes() {
        let bytes = declared("ISO-8859-15", b"\xA4\xA6\xA8\xB4\xB8\xBC\xBD\xBE\xE9");
        assert!(xml_text(&bytes).unwrap().ends_with("€ŠšŽžŒœŸé"));
    }

    #[test]
    fn a_latin1_csv_reads_below_11() {
        for text in ["9.0", "10.0"] {
            assert_eq!(
                csv_text(b"id,name\nx,ol\xE9\n", series(text)).unwrap(),
                "id,name\nx,olé\n"
            );
        }
    }

    #[test]
    fn a_latin1_csv_stays_unread_from_11_and_on_an_unknown_series() {
        for version in [series("11.0"), series("18.0"), None] {
            assert!(csv_text(b"id,name\nx,ol\xE9\n", version).is_none());
        }
    }

    #[test]
    fn a_utf8_csv_stays_borrowed_on_every_series() {
        for version in [series("9.0"), series("10.0"), series("11.0"), series("18.0"), None] {
            assert!(matches!(
                csv_text("x,olé\n".as_bytes(), version),
                Some(Cow::Borrowed(_))
            ));
        }
    }

    #[test]
    fn each_csv_cell_decodes_on_its_own() {
        assert_eq!(csv_text(b"a\xC3\xA9,\xE9", series("9.0")).unwrap(), "aé,é");
    }

    #[test]
    fn a_csv_cell_with_both_goes_latin1_whole() {
        assert_eq!(csv_text(b"\xC3\xA9\xE9,x", series("9.0")).unwrap(), "Ã©é,x");
    }

    #[test]
    fn a_quoted_comma_joins_a_csv_cell() {
        assert_eq!(csv_text(b"\"\xC3\xA9,\xE9\",x", series("9.0")).unwrap(), "\"Ã©,é\",x");
    }

    #[test]
    fn a_doubled_quote_keeps_the_csv_state() {
        assert_eq!(
            csv_text(b"\"a\"\"\xC3\xA9,\xE9\",x", series("9.0")).unwrap(),
            "\"a\"\"Ã©,é\",x"
        );
    }

    #[test]
    fn a_line_end_closes_a_csv_cell() {
        assert_eq!(csv_text(b"\xC3\xA9\r\n\xE9\n", series("9.0")).unwrap(), "é\r\né\n");
    }
}
