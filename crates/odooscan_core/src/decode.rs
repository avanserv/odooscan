//! Whether a file Odoo loads decodes, and the text of a data file as Odoo reads it. Odoo hands the
//! bytes of an XML file to `etree.parse` on every series, so libxml2 picks the encoding, not
//! Python: 9.0 `openerp/tools/convert.py:838,915`, 10.0 `odoo/tools/convert.py:835,899`, 11.0
//! `:784,836`, 18.0 `:659,715`, 20.0 `:712,792`.
//!
//! A CSV goes through `ustr` cell by cell below 11.0, UTF-8 first, then Latin-1, which cannot fail:
//! 8.0 `openerp/tools/convert.py:951`, 9.0 `:899` with `openerp/loglevels.py:28`, 10.0
//! `odoo/tools/convert.py:882` with `odoo/loglevels.py:28`. From 11.0 it is strict UTF-8: 11.0
//! `odoo/tools/convert.py:810` with `odoo/tools/pycompat.py:46-48,89-94`, 17.0 `:700`, 18.0 `:683`,
//! 20.0 `:741`.
//!
//! OD000 reports the place where Python or libxml2 stops decoding a file Odoo loads.

use std::borrow::Cow;
use std::sync::LazyLock;

use regex::bytes::Regex;
use ruff_text_size::TextSize;

use crate::facts::Pos;
use crate::python::Locator;
use crate::series::Series;

/// The first series that reads a data CSV as strict UTF-8 (`11.0`).
const CSV_UTF8: Series = Series { major: 11, minor: 0 };

/// A declaration opens the file or is none, so a BOM ahead of it hides it: libxml2 lets a BOM beat
/// the declaration (probed on lxml 5.2.1).
static DECLARED_ENCODING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?-u)^<\?xml\s+(?:version\s*=\s*(?:"[^"]*"|'[^']*')\s+)?encoding\s*=\s*["']([^"']*)["']"#).unwrap()
});

/// A coding comment as CPython reads one on either of the first two lines (`check_coding_spec` in
/// `Parser/tokenizer.c`, 2.7 to 3.14).
static CODING_SPEC: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?-u)^[ \t\x0C]*#.*?coding[:=][ \t]*([-\w.]+)").unwrap());

/// Where `bytes` stop being UTF-8, none when they all are.
pub fn invalid_utf8(bytes: &[u8]) -> Option<Pos> {
    let error = std::str::from_utf8(bytes).err()?;
    let valid = std::str::from_utf8(&bytes[..error.valid_up_to()]).unwrap_or_default();
    Some(Locator::new(valid).pos(TextSize::of(valid)))
}

/// Where Python stops reading a source file that is not UTF-8, none when it decodes or a coding
/// comment names another encoding, which Python reads it in instead. A UTF-8 BOM keeps it UTF-8
/// whatever the comment says.
pub fn python_refused(bytes: &[u8]) -> Option<Pos> {
    let pos = invalid_utf8(bytes)?;
    let declared = coding_label(bytes).filter(|_| !bytes.starts_with(b"\xEF\xBB\xBF"));
    declared.is_none_or(utf8_codec).then_some(pos)
}

/// The encoding a coding comment names on the first line, or on the second under a first that holds
/// a comment or nothing.
fn coding_label(bytes: &[u8]) -> Option<&[u8]> {
    let mut lines = bytes.split_inclusive(|&b| b == b'\n');
    let first = lines.next()?;
    if let Some(label) = coding_spec(first) {
        return Some(label);
    }
    let code = first.iter().copied().find(|b| !matches!(b, b' ' | b'\t' | b'\x0C'));
    matches!(code, None | Some(b'#' | b'\n' | b'\r'))
        .then(|| coding_spec(lines.next()?))
        .flatten()
}

fn coding_spec(line: &[u8]) -> Option<&[u8]> {
    CODING_SPEC.captures(line)?.get(1).map(|m| m.as_bytes())
}

/// Whether Python reads `label` as UTF-8: its codec's name and aliases once `codecs.lookup` folds
/// case and punctuation, and any `utf-8-` name the tokenizer cuts back to `utf-8`.
fn utf8_codec(label: &[u8]) -> bool {
    let mut name = String::new();
    for byte in label.to_ascii_lowercase() {
        if byte.is_ascii_alphanumeric() || byte == b'.' {
            name.push(char::from(byte));
        } else if !name.ends_with('_') {
            name.push('_');
        }
    }
    let aliases = ["utf_8", "utf8", "u8", "utf", "utf8_ucs2", "utf8_ucs4", "cp65001"];
    aliases.contains(&name.as_str()) || name.starts_with("utf_8_")
}

/// Where libxml2 stops reading an XML file that is not UTF-8, none when it decodes or libxml2 reads
/// it in another encoding: one a UTF-16 or UTF-32 BOM names, one a NUL among the first four bytes
/// stands for, or one the declaration names other than UTF-8.
pub fn xml_refused(bytes: &[u8]) -> Option<Pos> {
    let pos = invalid_utf8(bytes)?;
    let wide =
        bytes.starts_with(b"\xFE\xFF") || bytes.starts_with(b"\xFF\xFE") || bytes.iter().take(4).any(|&b| b == 0);
    let declared = DECLARED_ENCODING.captures(bytes).map(|c| c[1].to_ascii_uppercase());
    let utf8 = declared.is_none_or(|label| label == b"UTF-8" || label == b"UTF8");
    (!wide && utf8).then_some(pos)
}

/// Whether the Odoo of `series` reads a data CSV as strict UTF-8, an unknown series, which a
/// `10.saas~N` version parses to, taken as one that does.
pub fn csv_strict(series: Option<Series>) -> bool {
    series.is_none_or(|s| s >= CSV_UTF8)
}

pub(crate) fn latin1(byte: u8) -> char {
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

/// The text of a data CSV as `series` reads it, or none when it does not decode: an unread file
/// keeps the rules silent.
pub fn csv_text(bytes: &[u8], series: Option<Series>) -> Option<Cow<'_, str>> {
    if let Ok(text) = std::str::from_utf8(bytes) {
        return Some(Cow::Borrowed(text));
    }
    (!csv_strict(series)).then(|| Cow::Owned(csv_cells(bytes).map(csv_cell).collect()))
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

    fn at(line: u32, col: u32) -> Option<Pos> {
        Some(Pos { line, col })
    }

    #[test]
    fn invalid_utf8_points_at_the_first_bad_byte() {
        assert_eq!(invalid_utf8("a\né".as_bytes()), None);
        assert_eq!(invalid_utf8(b"ab\nc\xC3\xA9d\xE9"), at(2, 4));
        assert_eq!(invalid_utf8(b"\xE9"), at(1, 1));
    }

    #[test]
    fn python_refuses_what_no_other_coding_comment_covers() {
        let body = b"x = '\xE9'\n";
        let with = |head: &str| [head.as_bytes(), body].concat();
        assert_eq!(python_refused(body), at(1, 6));
        assert_eq!(python_refused(&with("# -*- coding: utf-8 -*-\n")), at(2, 6));
        assert_eq!(python_refused(&with("# vim: set fileencoding=UTF_8 :\n")), at(2, 6));
        assert_eq!(python_refused(&with("# coding: utf-8-sig\n")), at(2, 6));
        assert_eq!(python_refused(&with("# coding=utf8\n")), at(2, 6));
        assert_eq!(python_refused(&with("import os\n# coding: latin-1\n")), at(3, 6));
        assert_eq!(python_refused(&with("\u{FEFF}# coding: latin-1\n")), at(2, 6));
        assert_eq!(python_refused(&with("#coding : latin-1\n")), at(2, 6));
        for head in [
            "# -*- coding: latin-1 -*-\n",
            "\t# coding=iso-8859-15\n",
            "#!/usr/bin/env python\n# coding: cp1252\n",
            "\n# coding: latin-1\n",
        ] {
            assert_eq!(python_refused(&with(head)), None, "{head:?}");
        }
        assert_eq!(python_refused("x = 'é'\n".as_bytes()), None);
    }

    #[test]
    fn xml_refuses_only_what_it_reads_as_utf8() {
        assert_eq!(xml_refused(b"<odoo>\n<a>\xE9</a></odoo>"), at(2, 4));
        assert_eq!(xml_refused(&declared("UTF-8", b"\xE9")), at(1, 39));
        assert_eq!(xml_refused(&declared("utf8", b"\xE9")), at(1, 38));
        assert_eq!(xml_refused(b"<?xml version=\"1.0\"?><odoo>\xE9</odoo>"), at(1, 28));
        assert_eq!(
            xml_refused(b"\xEF\xBB\xBF<?xml version=\"1.0\" encoding=\"latin1\"?>\xE9"),
            at(1, 40)
        );
        assert_eq!(xml_refused(&declared("windows-1252", b"\xE9")), None);
        assert_eq!(xml_refused(&declared("ISO-8859-1", b"\xE9")), None);
        assert_eq!(xml_refused(b"\xFF\xFE<\x00o\x00"), None);
        assert_eq!(xml_refused(b"\xFE\xFF\x00<\x00o"), None);
        assert_eq!(xml_refused(b"<\x00?\x00\xE9\x00"), None);
        assert_eq!(xml_refused("<odoo>é</odoo>".as_bytes()), None);
    }

    #[test]
    fn a_csv_is_strict_from_11_and_on_an_unknown_series() {
        assert!(!csv_strict(series("10.0")));
        assert!(csv_strict(series("11.0")));
        assert!(csv_strict(None));
    }
}
