//! The messages of a Python file's `_()` and `_lt()` calls, as Odoo's export takes them into a
//! module's `.pot` template. The export runs Babel's extractor on 18.0 and Odoo's own from saas~18.2
//! (1c47e0c1d694, 20.0 `odoo/tools/babel/python_extractor.py`), both on the tokens of the file
//! rather than its tree, so each reads some parentheses as calls and some calls apart.

use ruff_python_ast::token::{Token, TokenKind};
use ruff_python_ast::visitor::{self, Visitor};
use ruff_python_ast::{Expr, FString, InterpolatedStringElement, StringLiteral};
use ruff_python_parser::parse_expression;
use ruff_text_size::{Ranged, TextSize};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::async_names::Parsed;
use crate::facts::Message;
use crate::python::Locator;
use crate::series::Series;

/// The first series whose export runs Odoo's extractor (1c47e0c1d694), which skips an f-string
/// that interpolates. Babel, before it, evaluates the f-string on Python before 3.12, where it is
/// one string token, and raises on its first name (Babel 2.10.3 `messages/extract.py:494-497`).
const OWN_EXTRACTOR: Series = Series { major: 18, minor: 2 };

/// The messages of the `parsed` file of a module of `series`: every text an extractor of some
/// series may take, and which of them every extractor takes.
pub(crate) fn extract(parsed: &Parsed, loc: &Locator, series: Option<Series>) -> Vec<Message> {
    let mut values = Values {
        loc,
        values: FxHashMap::default(),
        unreadable: FxHashSet::default(),
    };
    values.visit_body(&parsed.module.body);
    let mut reader = Reader {
        loc,
        values: &values.values,
        unreadable: &values.unreadable,
        evaluates_fstrings: series.is_none_or(|s| s < OWN_EXTRACTOR),
        reads_fstring_tokens: series.is_none_or(|s| s >= OWN_EXTRACTOR),
        open: vec![],
        last_name: None,
        last_outer_name: None,
        previous: None,
        before_previous: None,
        fstrings: 0,
        halts: false,
        out: vec![],
    };
    for token in parsed.tokens.iter() {
        reader.read(token);
    }
    let Reader { out, halts, .. } = reader;
    // The export logs the error and drops the rest of the file (18.0 `odoo/tools/translate.py:1416`).
    // Holding no message of the file certain keeps OD804 off each one it dropped.
    out.into_iter()
        .map(|m| Message {
            certain: m.certain && !halts,
            ..m
        })
        .collect()
}

/// The value of each string and f-string by its start, none for an f-string that interpolates.
struct Values<'a, 'l> {
    loc: &'l Locator<'a>,
    values: FxHashMap<TextSize, Option<String>>,
    /// The start of each constant f-string Odoo's extractor fails to read from 3.12.
    unreadable: FxHashSet<TextSize>,
}

impl Values<'_, '_> {
    /// `node` as Python reads it once `compile()` makes each line break of its text `\n`, which
    /// the parser keeps as the file writes it; none when it has no other line break.
    fn newlines_read(&self, node: &impl Ranged) -> Option<Expr> {
        let text = self.loc.text(node);
        if !text.contains('\r') {
            return None;
        }
        let read = text.replace("\r\n", "\n").replace('\r', "\n");
        parse_expression(&read).ok().map(|p| p.into_expr())
    }
}

/// Whether Odoo's extractor raises on the constant f-string `text` from 3.12, whose tokens give
/// each doubled brace once: it compiles the f-string they join back, which then no longer parses
/// (20.0 `odoo/tools/babel/python_extractor.py:188-194`). One that parses interpolates, and the
/// extractor only drops it.
fn collapse_breaks(text: &str) -> bool {
    if !text.contains("{{") && !text.contains("}}") {
        return false;
    }
    parse_expression(&text.replace("{{", "{").replace("}}", "}")).is_err()
}

/// The text of `fstring`, none when it interpolates.
fn fstring_value(fstring: &FString) -> Option<String> {
    let literal = |e: &InterpolatedStringElement| match e {
        InterpolatedStringElement::Literal(l) => Some(l.value.to_string()),
        InterpolatedStringElement::Interpolation(_) => None,
    };
    fstring.elements.iter().map(literal).collect()
}

impl<'a> Visitor<'a> for Values<'_, '_> {
    fn visit_string_literal(&mut self, literal: &'a StringLiteral) {
        let value = match self.newlines_read(literal) {
            Some(Expr::StringLiteral(read)) => read.value.to_string(),
            _ => literal.value.to_string(),
        };
        self.values.insert(literal.start(), Some(value));
    }

    fn visit_f_string(&mut self, fstring: &'a FString) {
        let value = match self.newlines_read(fstring) {
            Some(Expr::FString(read)) => read.value.f_strings().next().and_then(fstring_value),
            _ => fstring_value(fstring),
        };
        if value.is_some() && collapse_breaks(self.loc.text(fstring)) {
            self.unreadable.insert(fstring.start());
        }
        self.values.insert(fstring.start(), value);
        visitor::walk_f_string(self, fstring);
    }
}

/// What a name token is to the extractors: Python's tokenizer gives keywords as names.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Name {
    /// `_` or `_lt`, lazy for `_lt`.
    Translate {
        lazy: bool,
    },
    Definition,
    Other,
}

/// The first argument of a parenthesis whose last name before it is `_` or `_lt`, as read so far.
struct Argument {
    start: TextSize,
    lazy: bool,
    /// A call every extractor reads, so far as what precedes the parenthesis tells.
    call: bool,
    /// A call Babel reads, whose strings outside parentheses it evaluates.
    babel: bool,
    /// The argument has ended at a comma.
    done: bool,
    readings: Readings,
    /// Only plain strings so far, and how many.
    plain: bool,
    strings: usize,
    /// The f-string depth the parenthesis opens at.
    fstrings: usize,
}

impl Argument {
    /// Reads a token directly in the argument.
    fn direct(&mut self, token: &Token, value: Option<String>, translate: bool) {
        match token.kind() {
            TokenKind::String => self.string(value),
            TokenKind::FStringStart => {
                self.plain = false;
                if let Some(value) = value {
                    self.readings.constant(&value);
                }
            }
            TokenKind::Comma => self.done = true,
            TokenKind::Lpar => {
                self.plain = false;
                self.readings.whole_tail.clear();
                self.readings.split_tail.clear();
            }
            TokenKind::Rpar => {}
            _ => {
                self.plain = false;
                if translate {
                    self.readings.stopped = true;
                    self.readings.babel_stopped = true;
                }
            }
        }
    }

    /// Reads a string token, whose value is none when it is bytes.
    fn string(&mut self, value: Option<String>) {
        match value {
            Some(value) => {
                self.readings.string(&value);
                self.strings += 1;
            }
            None => self.plain = false,
        }
    }

    /// Reads a token in a replacement field of an f-string of the argument.
    fn replaced(&mut self, kind: TokenKind, value: Option<String>, translate: bool) {
        match kind {
            TokenKind::String => self.readings.replaced(&value.unwrap_or_default()),
            TokenKind::Lpar => self.readings.split_tail.clear(),
            // From 3.12 the comma ends the argument for both extractors, though not for Python.
            TokenKind::Comma => self.readings.split_done = true,
            _ => self.readings.babel_stopped |= translate,
        }
    }
}

/// The message each extractor makes of the strings of an argument. Python before 3.12 gives an
/// f-string as one token, which newer Babel and Odoo's extractor take when it interpolates
/// nothing; from 3.12 its tokens show the strings of its replacement fields, which both take too.
/// Odoo's extractor empties its buffer at each parenthesis.
#[derive(Default)]
struct Readings {
    /// Older Babel: plain strings, up to a `_` or `_lt` name.
    plain: String,
    stopped: bool,
    /// Older Babel from 3.12: plain strings and those of replacement fields, up to a `_` or `_lt`
    /// name.
    babel_split: String,
    babel_stopped: bool,
    /// Newer Babel before 3.12: plain strings and constant f-strings.
    whole: String,
    /// Newer Babel from 3.12: those and the strings of replacement fields.
    split: String,
    /// Odoo's extractor before 3.12: `whole` from the last parenthesis.
    whole_tail: String,
    /// Odoo's extractor from 3.12: `split` from the last parenthesis.
    split_tail: String,
    /// A comma in a replacement field has ended the argument for the readings from 3.12.
    split_done: bool,
}

impl Readings {
    fn string(&mut self, value: &str) {
        if !self.stopped {
            self.plain.push_str(value);
        }
        if !self.babel_stopped && !self.split_done {
            self.babel_split.push_str(value);
        }
        self.constant(value);
    }

    fn constant(&mut self, value: &str) {
        self.whole.push_str(value);
        self.whole_tail.push_str(value);
        if !self.split_done {
            self.split.push_str(value);
            self.split_tail.push_str(value);
        }
    }

    /// A string in a replacement field of an f-string of the argument.
    fn replaced(&mut self, value: &str) {
        if self.split_done {
            return;
        }
        self.split.push_str(value);
        self.split_tail.push_str(value);
        if !self.babel_stopped {
            self.babel_split.push_str(value);
        }
    }

    fn texts(self) -> Vec<String> {
        let mut texts: Vec<String> = vec![];
        let all = [
            self.whole,
            self.plain,
            self.split,
            self.whole_tail,
            self.split_tail,
            self.babel_split,
        ];
        for text in all {
            if !text.is_empty() && !texts.contains(&text) {
                texts.push(text);
            }
        }
        texts
    }
}

/// Where a token halts the export: in any argument, or directly in one Babel reads.
struct Halts {
    any: bool,
    babel: bool,
}

/// A parenthesis open at the token being read.
struct Open {
    argument: Option<Argument>,
}

struct Reader<'a, 'l> {
    loc: &'l Locator<'a>,
    values: &'l FxHashMap<TextSize, Option<String>>,
    unreadable: &'l FxHashSet<TextSize>,
    open: Vec<Open>,
    last_name: Option<Name>,
    /// The last name outside f-strings, which Python reads as one token before 3.12.
    last_outer_name: Option<Name>,
    previous: Option<Token>,
    before_previous: Option<Token>,
    fstrings: usize,
    /// Babel evaluates an f-string of a call, as the module's series may run it on Python before
    /// 3.12.
    evaluates_fstrings: bool,
    /// Odoo's extractor reads an f-string of a call by its tokens, as the module's series may run
    /// it on Python 3.12 or later.
    reads_fstring_tokens: bool,
    /// A token stops the export of the file.
    halts: bool,
    out: Vec<Message>,
}

impl Reader<'_, '_> {
    fn read(&mut self, token: &Token) {
        let kind = token.kind();
        if matches!(kind, TokenKind::NonLogicalNewline | TokenKind::Comment) {
            self.step(token);
            return;
        }
        self.argument_token(token);
        match kind {
            TokenKind::Lpar => self.open(token),
            TokenKind::Rpar => {
                if let Some(Open { argument: Some(a) }) = self.open.pop() {
                    self.finish(a);
                }
            }
            TokenKind::FStringStart => self.fstrings += 1,
            TokenKind::FStringEnd => self.fstrings = self.fstrings.saturating_sub(1),
            _ => {}
        }
        if let Some(name) = self.name(token) {
            self.last_name = Some(name);
            if self.fstrings == 0 {
                self.last_outer_name = Some(name);
            }
        }
        self.step(token);
    }

    fn step(&mut self, token: &Token) {
        self.before_previous = self.previous.replace(*token);
    }

    fn name(&self, token: &Token) -> Option<Name> {
        let kind = token.kind();
        if kind == TokenKind::Identifier {
            let lazy = match self.loc.text(token) {
                "_" => false,
                "_lt" => true,
                _ => return Some(Name::Other),
            };
            return Some(Name::Translate { lazy });
        }
        if matches!(kind, TokenKind::Def | TokenKind::Class) {
            return Some(Name::Definition);
        }
        (kind.is_keyword() || matches!(kind, TokenKind::Async | TokenKind::Await)).then_some(Name::Other)
    }

    fn open(&mut self, token: &Token) {
        let translate = |name| match name {
            Some(Name::Translate { lazy }) => Some(lazy),
            _ => None,
        };
        let lazy = translate(self.last_name).or_else(|| translate(self.last_outer_name).filter(|_| self.fstrings == 0));
        let argument = lazy.map(|lazy| self.argument(token, lazy));
        self.open.push(Open { argument });
    }

    fn argument(&self, token: &Token, lazy: bool) -> Argument {
        let named = self.previous.and_then(|p| self.name(&p));
        let defined = self.before_previous.and_then(|p| self.name(&p)) == Some(Name::Definition);
        let nested = self.open.iter().any(|o| o.argument.is_some());
        let babel = matches!(named, Some(Name::Translate { .. })) && !defined && self.fstrings == 0;
        let call = babel && !nested;
        let start = if call {
            self.previous.map_or(token.start(), |p| p.start())
        } else {
            token.start()
        };
        Argument {
            start,
            lazy,
            call,
            babel,
            done: false,
            readings: Readings::default(),
            plain: true,
            strings: 0,
            fstrings: self.fstrings,
        }
    }

    /// Reads `token` as a part of the first argument of the innermost parenthesis, when it is one
    /// whose argument is being read and the token sits directly in it, or in an f-string there.
    /// Any argument halts the export on a token the extractor cannot join with text: a bytes
    /// string, an f-string Babel evaluates and that interpolates, or one whose doubled braces
    /// Odoo's extractor cannot read.
    fn argument_token(&mut self, token: &Token) {
        let fstrings = self.fstrings;
        let translate = matches!(self.name(token), Some(Name::Translate { .. }));
        let value = self.values.get(&token.start()).cloned().flatten();
        let halts = self.halts_on(token);
        let Some(Open { argument: Some(a) }) = self.open.last_mut() else {
            return;
        };
        let direct = a.fstrings == fstrings;
        self.halts |= halts.any || (halts.babel && direct && a.babel);
        if a.done {
            return;
        }
        if direct {
            a.direct(token, value, translate);
        } else {
            a.replaced(token.kind(), value, translate);
        }
    }

    /// Whether `token` in an argument halts the export there, and whether it does in one Babel
    /// reads, directly in it.
    fn halts_on(&self, token: &Token) -> Halts {
        let entry = self.values.get(&token.start());
        let interpolates = token.kind() == TokenKind::FStringStart && matches!(entry, Some(None));
        let bytes = token.string_flags().is_some_and(|f| f.is_byte_string());
        let unreadable = self.reads_fstring_tokens && self.unreadable.contains(&token.start());
        Halts {
            any: bytes || unreadable,
            babel: interpolates && self.evaluates_fstrings,
        }
    }

    /// Babel and Odoo's extractor skip an empty message. A certain call has one text, as its
    /// strings are plain and its parenthesis opens none.
    fn finish(&mut self, a: Argument) {
        let pos = self.loc.pos(a.start);
        let certain = a.call && a.plain && a.strings > 0;
        self.out.extend(a.readings.texts().into_iter().map(|text| Message {
            text,
            pos,
            lazy: a.lazy,
            certain,
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::async_names;

    /// The messages of `src`, each with whether it is certain and lazy.
    fn read(src: &str) -> Vec<(String, bool, bool)> {
        read_in(src, None)
    }

    fn read_in(src: &str, series: Option<Series>) -> Vec<(String, bool, bool)> {
        let parsed = async_names::parse(src, series).unwrap_or_else(|e| panic!("{src}: {e}"));
        let messages = extract(&parsed, &Locator::new(src), series);
        messages.into_iter().map(|m| (m.text, m.certain, m.lazy)).collect()
    }

    fn certain(text: &str) -> (String, bool, bool) {
        (text.to_owned(), true, false)
    }

    fn known(text: &str) -> (String, bool, bool) {
        (text.to_owned(), false, false)
    }

    #[test]
    fn a_call_of_plain_strings_is_certain_at_its_name() {
        let src = "def f(self, n):\n    _(\"Stall opened\")\n    self.env._(\"Stall %s\", n)\n    _(\n        \"Stall \"  # joined\n        \"rented\"\n    )\n";
        assert_eq!(
            read(src),
            [certain("Stall opened"), certain("Stall %s"), certain("Stall rented")]
        );
        let parsed = async_names::parse(src, None).unwrap();
        let positions: Vec<_> = extract(&parsed, &Locator::new(src), None)
            .into_iter()
            .map(|m| m.pos)
            .collect();
        assert_eq!(positions[1], crate::facts::Pos { line: 3, col: 14 });
    }

    #[test]
    fn lt_is_lazy() {
        assert_eq!(read("RENT = _lt(\"Rent\")\n"), [("Rent".to_owned(), true, true)]);
    }

    #[test]
    fn a_first_argument_of_more_than_plain_strings_is_only_known() {
        for (src, texts) in [
            ("_((\"Lit\"))", vec!["Lit"]),
            ("_(\"Stall \" + name)", vec!["Stall "]),
            ("_(\"Stall of %s\" % name)", vec!["Stall of %s"]),
            ("_(f\"Dusted\")", vec!["Dusted"]),
            ("_(f\"Shelf {x}\")", vec![]),
            ("_(\"a\" f\"b\" \"c\")", vec!["abc", "ac"]),
            ("_(\"a\" + foo(x) + \"c\")", vec!["ac", "c"]),
            ("_(\"x\" + _(\"y\"))", vec!["y", "x"]),
            ("_(\"a\" + f\"{b['c']}\")", vec!["a", "ac"]),
            ("_(\"a\" + f\"{b(['c'])}\")", vec!["a"]),
            ("_(f\"b\" f\"{x['c']}\")", vec!["b", "bc", "c"]),
            ("_(\"a\" f\"{x, 'b'}\" \"c\")", vec!["ac", "a"]),
            ("_(\"a\" f\"{_(x)}{'b'}\")", vec!["a", "ab", "b"]),
            ("_(msg=\"Kw\")", vec!["Kw"]),
        ] {
            let found = read(src);
            assert!(found.iter().all(|m| !m.1), "{src}: {found:?}");
            let found: Vec<_> = found.iter().map(|m| m.0.as_str()).collect();
            assert_eq!(found, texts, "{src}");
        }
    }

    #[test]
    fn a_call_nested_in_another_or_in_an_f_string_is_only_known() {
        assert_eq!(read("_(\"a %s\", _(\"b\"))"), [known("b"), certain("a %s")]);
        assert_eq!(read("x = f\"{_('Inside')}\""), [known("Inside")]);
        assert_eq!(read("x = (_\n(\"Apart\"))"), [known("Apart")]);
    }

    #[test]
    fn a_parenthesis_after_a_call_with_no_name_between_is_known() {
        let src = "x = {\"name\": _(\"SQL\"), \"domain\": [(\"profile\", \"in\", ids)]}";
        assert_eq!(read(src), [certain("SQL"), known("profile")]);
        assert_eq!(read("x = _(\"a\") or (\"b\")"), [certain("a")]);
        assert_eq!(read("x = _(\"a\") + f\"{y}\" + (\"b\")"), [certain("a"), known("b")]);
    }

    #[test]
    fn a_definition_and_an_empty_call_have_no_message() {
        assert_eq!(read("def _(self, text=\"Label\"):\n    return text\n"), []);
        assert_eq!(read("_()\n_(\"\")\n"), []);
    }

    #[test]
    fn a_call_of_bytes_leaves_nothing_certain() {
        let src = "_(\"Before\")\n_(b\"raw\")\n_(\"After\")\n";
        assert_eq!(read(src), [known("Before"), known("After")]);
        let src = "_(\"Before\")\n_(\"x %s\", b\"y\")\n_(\"After\")\n";
        assert_eq!(read(src), [known("Before"), known("x %s"), known("After")]);
        let src = "_(\"Before\")\n_(\"x %s\", f\"{b'y'}\")\n";
        assert_eq!(read(src), [known("Before"), known("x %s")]);
    }

    #[test]
    fn an_interpolating_f_string_babel_evaluates_leaves_nothing_certain() {
        let first = "_(f\"Bad {x}\")\n_(\"Done\")\n";
        let later = "_(\"Bad %s\", f\"{x}\")\n_(\"Done\")\n";
        for series in [None, Series::parse("16.0"), Series::parse("saas~18.1")] {
            assert_eq!(read_in(first, series), [known("Done")], "{series:?}");
            assert_eq!(read_in(later, series), [known("Bad %s"), known("Done")], "{series:?}");
        }
        let own = Series::parse("saas~18.2");
        assert_eq!(read_in(first, own), [certain("Done")]);
        assert_eq!(read_in(later, own), [certain("Bad %s"), certain("Done")]);
    }

    #[test]
    fn a_doubled_brace_odoo_cannot_read_leaves_nothing_certain() {
        for src in [
            "_(\"Before\")\n_(f\"Close with }}\")\n_(\"After\")\n",
            "_(\"Before\")\n_(f\"Use {{}} here\")\n_(\"After\")\n",
            "_(\"Before\")\n_(\"x %s\", f\"{f'}}'}\")\n_(\"After\")\n",
        ] {
            for series in [None, Series::parse("saas~18.2"), Series::parse("20.0")] {
                let found = read_in(src, series);
                assert!(found.iter().all(|m| !m.1), "{src} {series:?}: {found:?}");
            }
        }
        let babel = "_(f\"Close with }}\")\n_(\"After\")\n";
        assert_eq!(
            read_in(babel, Series::parse("18.0")),
            [known("Close with }"), certain("After")]
        );
        for src in [
            "_(\"Before\")\n_(f\"{{x}}\")\nf\"}}\"\n_(\"After\")\n",
            "_(\"Before\")\n_(\"x %s\", g(f\"}}\"))\n_(\"After\")\n",
        ] {
            let found = read_in(src, Series::parse("saas~18.2"));
            assert_eq!(found.first(), Some(&certain("Before")), "{src}");
            assert_eq!(found.last(), Some(&certain("After")), "{src}");
        }
    }

    #[test]
    fn an_interpolating_f_string_babel_does_not_evaluate_leaves_the_file_certain() {
        for src in [
            "_(\"Bad %s\" % (f\"{x}\",))\n_(\"Done\")\n",
            "_(\"Bad %s\", g(f\"{x}\"))\n_(\"Done\")\n",
            "y = f\"{x}\"\n_(\"Done\")\n",
            "z = f\"{_(x)}\"\n_(\"Done\")\n",
        ] {
            let found = read_in(src, Series::parse("16.0"));
            assert_eq!(found.last(), Some(&certain("Done")), "{src}");
        }
    }

    #[test]
    fn a_line_break_of_a_crlf_file_is_read_as_python_reads_it() {
        let src = "_(\"\"\"Two\r\nlines\"\"\")\r\n_(f\"\"\"Two\r\nmore\"\"\")\r\n_(\"\"\"Kept\\r\\n\"\"\")\r\n";
        assert_eq!(
            read(src),
            [certain("Two\nlines"), known("Two\nmore"), certain("Kept\r\n")]
        );
    }
}
