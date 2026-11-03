//! Python 2 syntax, which 9.0 and 10.0 run on and Ruff's parser refuses. Each form 9.0 and 10.0
//! core write is rewritten at the same length into Python 3 that parses alike, so that a file is
//! told apart from one that does not parse in either Python. This approximates a Python 2 grammar:
//! a file that also holds Python 3-only syntax passes.

use std::borrow::Cow;

use ruff_python_ast::token::{Token, TokenKind};
use ruff_python_parser::{Mode, parse_unchecked};
use ruff_text_size::{Ranged, TextRange, TextSize};

use crate::async_names;
use crate::facts::Pos;
use crate::python::{Locator, opens_statement};
use crate::series::Series;

/// The first series on Python 3: `debian/control:7` on 9.0 and 10.0 names
/// `X-Python-Version: 2.7`, where `setup.py:60` on 11.0 asks for `>=3.5`.
const PYTHON3: Series = Series { major: 11, minor: 0 };

/// Whether a module of `series` runs on Python 2. An unknown series does not, so a file that only
/// parses as Python 2 keeps its finding there.
pub(crate) fn runs_on(series: Option<Series>) -> bool {
    series.is_some_and(|series| series < PYTHON3)
}

/// The first error of `src` read as Python 2 with its message, none when it parses so. The error
/// sits where the file has it, though tabs in the indentation are expanded to parse.
pub(crate) fn remaining_error(src: &str, series: Option<Series>) -> Option<(Pos, String)> {
    let (expanded, deltas) = expand_tabs(src);
    let rewritten = rewrite(&expanded);
    let error = async_names::parse(&rewritten, series).err()?;
    let mut pos = Locator::new(&rewritten).pos(error.location.start());
    let delta = deltas.get(pos.line as usize - 1).copied().unwrap_or_default();
    pos.col = pos.col.saturating_sub(delta).max(1);
    Some((pos, error.error.to_string()))
}

/// `src` with the tabs of each indentation expanded to stops of 8, as Python 2 reads them where
/// Python 3 refuses a mix of tabs and spaces, and the columns each line gains.
fn expand_tabs(src: &str) -> (Cow<'_, str>, Vec<u32>) {
    if !src.contains('\t') {
        return (Cow::Borrowed(src), vec![]);
    }
    let mut expanded = String::with_capacity(src.len());
    let mut deltas = vec![];
    for line in src.split_inclusive('\n') {
        let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
        let width = line[..indent]
            .chars()
            .fold(0, |width, c| if c == '\t' { (width / 8 + 1) * 8 } else { width + 1 });
        expanded.extend(std::iter::repeat_n(' ', width));
        expanded.push_str(&line[indent..]);
        deltas.push((width - indent) as u32);
    }
    (Cow::Owned(expanded), deltas)
}

/// One rewrite: the range of the file and the Python 3 of the same length that takes its place.
type Edit = (TextRange, Cow<'static, str>);

/// `code` with each Python 2 form its tokens show replaced by Python 3 of the same length.
fn rewrite(code: &str) -> String {
    let parsed = parse_unchecked(code, Mode::Module.into());
    let tokens: Vec<&Token> = parsed.tokens().iter().filter(|t| !t.kind().is_trivia()).collect();
    let mut forms = Forms {
        code,
        print_function: imports_print_function(code, &tokens),
        tokens,
        backtick_open: false,
    };
    let mut out = code.to_owned();
    for i in 0..forms.tokens.len() {
        for (range, text) in forms.edits(i) {
            out.replace_range(std::ops::Range::<usize>::from(range), &text);
        }
    }
    out
}

/// The Python 2 forms of a file, read token by token.
struct Forms<'a> {
    code: &'a str,
    tokens: Vec<&'a Token>,
    print_function: bool,
    backtick_open: bool,
}

impl Forms<'_> {
    /// The rewrites the token at `i` opens.
    fn edits(&mut self, i: usize) -> Vec<Edit> {
        let token = self.tokens[i];
        let edit = match token.kind() {
            TokenKind::Identifier => self
                .statement(i)
                .or_else(|| self.long_suffix(i))
                .or_else(|| self.ur_prefix(i)),
            // `rais= E, "message"`, as for `print`.
            TokenKind::Raise => raises_with_comma(&self.tokens[i + 1..]).then(|| (last(token.range()), "=".into())),
            TokenKind::Less => self.not_equal(i),
            TokenKind::Unknown => self.unknown(token),
            // Python 2 lets a name be bound to either.
            TokenKind::True | TokenKind::False => Some((token.range(), self.text(token).to_ascii_uppercase().into())),
            TokenKind::Def | TokenKind::Lambda => return self.tuple_parameters(i),
            _ => None,
        };
        edit.into_iter().collect()
    }

    fn text(&self, token: &Token) -> &str {
        &self.code[token.range()]
    }

    fn prev(&self, i: usize) -> Option<&Token> {
        i.checked_sub(1).map(|i| self.tokens[i])
    }

    fn next(&self, i: usize) -> Option<&Token> {
        self.tokens.get(i + 1).copied()
    }

    /// A `print` or `exec` statement, as an assignment, `prin= x`, which takes any operand the
    /// statement takes.
    fn statement(&self, i: usize) -> Option<Edit> {
        let token = self.tokens[i];
        let text = self.text(token);
        let keyword = text == "exec" || (text == "print" && !self.print_function);
        let opens = self
            .prev(i)
            .is_none_or(|t| opens_statement(t.kind()) || t.kind() == TokenKind::Colon);
        let operand = self.next(i).is_some_and(|t| starts_operand(t.kind()));
        (keyword && opens && operand).then(|| (last(token.range()), "=".into()))
    }

    /// A long integer: `10L` becomes `100`, though not `1e5L`, which Python 2 refuses too.
    fn long_suffix(&self, i: usize) -> Option<Edit> {
        let token = self.tokens[i];
        let integer =
            |t: &Token| t.kind() == TokenKind::Int || (t.kind() == TokenKind::Unknown && is_octal(self.text(t)));
        let suffix = self.prev(i).is_some_and(|t| t.end() == token.start() && integer(t));
        (suffix && self.text(token).eq_ignore_ascii_case("l")).then(|| (token.range(), "0".into()))
    }

    /// `ur"x"` becomes `+r"x"`.
    fn ur_prefix(&self, i: usize) -> Option<Edit> {
        let token = self.tokens[i];
        let string = self
            .next(i)
            .is_some_and(|t| t.kind() == TokenKind::String && t.start() == token.end());
        (string && self.text(token).eq_ignore_ascii_case("ur"))
            .then(|| (TextRange::at(token.start(), TextSize::new(1)), "+".into()))
    }

    /// `<>` becomes `!=`.
    fn not_equal(&self, i: usize) -> Option<Edit> {
        let token = self.tokens[i];
        let greater = self
            .next(i)
            .is_some_and(|t| t.kind() == TokenKind::Greater && t.start() == token.end());
        greater.then(|| {
            (
                TextRange::new(token.start(), token.end() + TextSize::new(1)),
                "!=".into(),
            )
        })
    }

    /// A backtick, which opens or closes a `repr`, or an octal integer.
    fn unknown(&mut self, token: &Token) -> Option<Edit> {
        let text = self.text(token);
        if text == "`" {
            self.backtick_open = !self.backtick_open;
            return Some((token.range(), if self.backtick_open { "(" } else { ")" }.into()));
        }
        is_octal(text).then(|| (TextRange::at(token.start(), TextSize::new(1)), "1".into()))
    }

    /// The parentheses of the tuple parameters of a `def` or a `lambda` at `i`, each as a space.
    fn tuple_parameters(&self, i: usize) -> Vec<Edit> {
        let parens = match self.tokens[i].kind() {
            TokenKind::Lambda => tuple_parens(&self.tokens[i..], TokenKind::Colon),
            _ if self.tokens.get(i + 2).is_some_and(|t| t.kind() == TokenKind::Lpar) => {
                tuple_parens(&self.tokens[i + 2..], TokenKind::Rpar)
            }
            _ => vec![],
        };
        parens.into_iter().map(|range| (range, " ".into())).collect()
    }
}

/// The last character of `range`.
fn last(range: TextRange) -> TextRange {
    TextRange::new(range.end() - TextSize::new(1), range.end())
}

/// Whether a file imports `print_function` from `__future__`, which makes `print` a name.
fn imports_print_function(code: &str, tokens: &[&Token]) -> bool {
    tokens.windows(3).enumerate().any(|(i, window)| {
        let [from, module, import] = window else { return false };
        from.kind() == TokenKind::From
            && &code[module.range()] == "__future__"
            && import.kind() == TokenKind::Import
            && tokens[i + 3..]
                .iter()
                .take_while(|t| t.kind() != TokenKind::Newline)
                .any(|t| &code[t.range()] == "print_function")
    })
}

/// Whether a token of `kind` may open the operand of `print` or `exec`, after which Python 3 reads
/// neither as a name. `>>` may not: `print >> f, x` parses in both.
fn starts_operand(kind: TokenKind) -> bool {
    use TokenKind as K;
    matches!(
        kind,
        K::Identifier
            | K::Int
            | K::Float
            | K::Complex
            | K::String
            | K::FStringStart
            | K::TStringStart
            | K::Lpar
            | K::Lsqb
            | K::Lbrace
            | K::Minus
            | K::Plus
            | K::Tilde
            | K::Not
            | K::Lambda
            | K::None
            | K::True
            | K::False
            | K::Ellipsis
            | K::Async
            | K::Await
            | K::Unknown
    ) || kind.is_soft_keyword()
}

/// Whether a token Ruff does not read is a Python 2 octal integer, `0755`, which Python 3 spells
/// `0o755`. Its length is kept by reading it as a decimal with a leading `1`.
fn is_octal(text: &str) -> bool {
    text.len() > 1 && text.starts_with('0') && text.bytes().all(|b| (b'0'..=b'7').contains(&b))
}

/// Whether the statement after `raise` lists its parts with a comma, `raise E, "message"`.
fn raises_with_comma(rest: &[&Token]) -> bool {
    let mut depth = 0usize;
    for token in rest {
        match token.kind() {
            TokenKind::Lpar | TokenKind::Lsqb | TokenKind::Lbrace => depth += 1,
            TokenKind::Rpar | TokenKind::Rsqb | TokenKind::Rbrace => depth = depth.saturating_sub(1),
            TokenKind::Comma if depth == 0 => return true,
            TokenKind::Newline | TokenKind::Semi | TokenKind::EndOfFile if depth == 0 => return false,
            _ => {}
        }
    }
    false
}

/// The parentheses of the tuple parameters in a parameter list, `def f(self, (a, b))` or
/// `lambda (a, b): a`. `tokens` starts at the `(` of a `def` or at the `lambda`, and the list ends
/// at the `end` that closes it.
fn tuple_parens(tokens: &[&Token], end: TokenKind) -> Vec<TextRange> {
    let mut list = ParameterList {
        end,
        // A `def` opens the list with its `(`.
        base: usize::from(end == TokenKind::Rpar),
        depth: 0,
        in_tuple: false,
        parens: vec![],
    };
    for (i, token) in tokens.iter().enumerate() {
        let after = i.checked_sub(1).map(|i| tokens[i].kind());
        if !list.step(token, after) {
            break;
        }
    }
    list.parens
}

/// A parameter list read token by token, with the parentheses of its tuple parameters.
struct ParameterList {
    end: TokenKind,
    /// The depth of the list itself.
    base: usize,
    depth: usize,
    in_tuple: bool,
    parens: Vec<TextRange>,
}

impl ParameterList {
    /// Moves past `token`, which follows a token of kind `after`, and says whether the list goes on.
    fn step(&mut self, token: &Token, after: Option<TokenKind>) -> bool {
        match token.kind() {
            TokenKind::Lpar => self.open(token.range(), after),
            TokenKind::Rpar => self.close(token.range()),
            TokenKind::Newline | TokenKind::EndOfFile => false,
            kind => kind != self.end || self.depth != self.base,
        }
    }

    fn open(&mut self, range: TextRange, after: Option<TokenKind>) -> bool {
        let starts = matches!(after, Some(TokenKind::Lpar | TokenKind::Comma | TokenKind::Lambda));
        self.in_tuple |= starts && self.depth == self.base;
        if self.in_tuple {
            self.parens.push(range);
        }
        self.depth += 1;
        true
    }

    fn close(&mut self, range: TextRange) -> bool {
        self.depth = self.depth.saturating_sub(1);
        if self.in_tuple {
            self.parens.push(range);
            self.in_tuple = self.depth > self.base;
        }
        self.depth > 0 || self.end != TokenKind::Rpar
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEN: Option<Series> = Some(Series { major: 10, minor: 0 });

    fn parses(src: &str) -> bool {
        remaining_error(src, TEN).is_none()
    }

    #[test]
    fn each_form_parses_once_rewritten() {
        for src in [
            "print 'x', y\n",
            "print\nprint >> f, 'x'\n",
            "if a: print a\n",
            "def f():\n    print -1; print (a), b\n    exec code in ns\n",
            "raise ValueError, 'x'\n",
            "if a: raise E, 'x' % (a, b), tb\n",
            "if a <> b:\n    pass\n",
            "x = 0755\n",
            "x = 0755L + 10L + 0x1Fl + 0L\n",
            "x = ur'a' UR\"b\"\n",
            "x = `y` + `z`\n",
            "def f(self, (a, (b, c)), d=(1, 2)):\n    pass\n",
            "x = lambda (k, v), w: k\n",
            "y = map(lambda (k, v): k, items)\n",
            "def f(True=1):\n    False = 0\n",
            "if a:\n\tx = 1\n        y = 2\n",
            "async = 1\n",
        ] {
            assert!(parses(src), "{src}: {:?}", remaining_error(src, TEN));
        }
    }

    #[test]
    fn python_2_refuses_these_too() {
        for src in [
            "None = 1\n",
            "x = 089\n",
            "x = 1e5L\n",
            "x = (\n",
            "from __future__ import print_function\nprint 'x'\n",
        ] {
            assert!(!parses(src), "{src}");
        }
    }

    #[test]
    fn the_error_is_the_first_one_left() {
        let (pos, _) = remaining_error("print \"x\"\nx = (", TEN).unwrap();
        assert_eq!(pos.line, 2);
    }

    #[test]
    fn a_column_past_expanded_tabs_maps_back() {
        let (pos, _) = remaining_error("if a:\n\t  x = 089\n", TEN).unwrap();
        assert_eq!((pos.line, pos.col), (2, 8));
    }

    #[test]
    fn only_9_0_and_10_0_run_on_python_2() {
        assert!(runs_on(Series::parse("9.0")) && runs_on(TEN));
        assert!(!runs_on(Series::parse("11.0")) && !runs_on(None));
    }
}
