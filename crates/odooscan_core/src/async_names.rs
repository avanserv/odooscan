//! `async` and `await` where the Python of a series holds them as names, which Ruff's lexer never
//! does: it reserves both at any target.

use std::borrow::Cow;

use ruff_python_ast::name::Name;
use ruff_python_ast::token::{Token, TokenKind};
use ruff_python_ast::visitor::transformer::{self, Transformer};
use ruff_python_ast::{self as ast, ExceptHandler, Expr, ModModule, Stmt};
use ruff_python_parser::{Mode, ParseError, parse_module, parse_unchecked};
use ruff_text_size::{Ranged, TextRange};

use crate::series::Series;

/// The first series whose floor is Python 3.7, which reserves `async` and `await`: `setup.py:58` on
/// 15.0 asks for `>=3.7` where `setup.py:59` on saas~14.5 asks for `>=3.6`.
pub(crate) const ASYNC_RESERVED: Series = Series { major: 15, minor: 0 };

/// `src` as a module of `series`, with both words spelled as `src` spells them wherever
/// `names_async` renamed them, so that no fact holds a name the file does not.
pub fn parse(src: &str, series: Option<Series>) -> Result<ModModule, ParseError> {
    let named = names_async(src, series);
    let mut module = parse_module(&named)?.into_syntax();
    if named != src {
        Spelling { src }.visit_body(&mut module.body);
    }
    Ok(module)
}

/// `code` as a module of `series` reads it. Where both words are names their tokens are renamed
/// before the parse that counts, in uppercase to keep every offset, which is how a name is read back
/// from `code`. An unknown series takes the names reading, so a finding never rests on a Python the
/// module may not run on.
pub(crate) fn names_async(code: &str, series: Option<Series>) -> Cow<'_, str> {
    let reserved = series.is_some_and(|series| series >= ASYNC_RESERVED);
    if reserved || !(code.contains("async") || code.contains("await")) {
        return Cow::Borrowed(code);
    }
    let parsed = parse_unchecked(code, Mode::Module.into());
    let tokens = parsed.tokens();
    let mut named = code.to_owned();
    let mut scope = AsyncScope::default();
    for (i, token) in tokens.iter().enumerate() {
        let inside = scope.step(token.kind(), tokens.get(i + 1).map(Token::kind));
        if !inside && matches!(token.kind(), TokenKind::Async | TokenKind::Await) {
            named[token.range()].make_ascii_uppercase();
        }
    }
    Cow::Owned(named)
}

/// Where Python before 3.7 reserves both words: from `async def` through its block, which a nested
/// `async def` does not extend.
#[derive(Default)]
struct AsyncScope {
    indent: usize,
    /// The indent of the open `async def`.
    open: Option<usize>,
    /// Whether the open `async def` has passed a logical newline: its signature sits at its indent
    /// without ending it.
    past_signature: bool,
}

impl AsyncScope {
    /// Moves past a token of `kind`, and says whether both words are reserved at it.
    fn step(&mut self, kind: TokenKind, next: Option<TokenKind>) -> bool {
        match kind {
            TokenKind::Indent => self.indent += 1,
            TokenKind::Dedent => self.indent = self.indent.saturating_sub(1),
            TokenKind::Newline => self.past_signature = true,
            TokenKind::NonLogicalNewline | TokenKind::Comment => {}
            _ => self.code_token(kind, next),
        }
        self.open.is_some()
    }

    fn code_token(&mut self, kind: TokenKind, next: Option<TokenKind>) {
        if self.past_signature && self.open.is_some_and(|indent| self.indent <= indent) {
            self.open = None;
        }
        if self.open.is_none() && kind == TokenKind::Async && next == Some(TokenKind::Def) {
            self.open = Some(self.indent);
            self.past_signature = false;
        }
    }
}

/// Puts back the spelling of `src` in a tree parsed from the renamed text. Patterns and type
/// parameters are left out: the Pythons that have them reserve both words.
struct Spelling<'a> {
    src: &'a str,
}

impl Spelling<'_> {
    /// A name the parser normalized differs from `src` by more than case, and a real `ASYNC` does
    /// not differ at all.
    fn name(&self, id: &mut Name, range: TextRange) {
        let word = &self.src[range];
        if word != id.as_str() && word.eq_ignore_ascii_case(id) {
            *id = Name::new(word);
        }
    }

    fn identifier(&self, identifier: &mut ast::Identifier) {
        self.name(&mut identifier.id, identifier.range);
    }
}

impl Transformer for Spelling<'_> {
    fn visit_stmt(&self, stmt: &mut Stmt) {
        match stmt {
            Stmt::FunctionDef(def) => self.identifier(&mut def.name),
            Stmt::ClassDef(def) => self.identifier(&mut def.name),
            Stmt::Global(s) => s.names.iter_mut().for_each(|name| self.identifier(name)),
            Stmt::Nonlocal(s) => s.names.iter_mut().for_each(|name| self.identifier(name)),
            Stmt::ImportFrom(s) => s.module.iter_mut().for_each(|module| self.identifier(module)),
            _ => {}
        }
        transformer::walk_stmt(self, stmt);
    }

    fn visit_expr(&self, expr: &mut Expr) {
        match expr {
            Expr::Name(name) => self.name(&mut name.id, name.range),
            Expr::Attribute(attribute) => self.identifier(&mut attribute.attr),
            _ => {}
        }
        transformer::walk_expr(self, expr);
    }

    fn visit_parameter(&self, parameter: &mut ast::Parameter) {
        self.identifier(&mut parameter.name);
        transformer::walk_parameter(self, parameter);
    }

    fn visit_keyword(&self, keyword: &mut ast::Keyword) {
        keyword.arg.iter_mut().for_each(|arg| self.identifier(arg));
        transformer::walk_keyword(self, keyword);
    }

    fn visit_alias(&self, alias: &mut ast::Alias) {
        self.identifier(&mut alias.name);
        alias.asname.iter_mut().for_each(|name| self.identifier(name));
    }

    fn visit_except_handler(&self, handler: &mut ExceptHandler) {
        let ExceptHandler::ExceptHandler(h) = handler;
        h.name.iter_mut().for_each(|name| self.identifier(name));
        transformer::walk_except_handler(self, handler);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAMES: Option<Series> = Some(Series { major: 13, minor: 0 });

    fn tree(code: &str, series: Option<Series>) -> String {
        format!("{:?}", parse(code, series).unwrap_or_else(|e| panic!("{code}: {e}")))
    }

    #[test]
    fn no_renamed_word_is_left_in_any_place_a_name_sits() {
        for code in [
            "def async():\n    pass",
            "class async:\n    pass",
            "def f():\n    global async",
            "def f():\n    nonlocal async",
            "from async import a",
            "from a.async import b",
            "x = async",
            "x = a.async",
            "x = f'{async}'",
            "x = a.async.b",
            "def f(async):\n    pass",
            "def f(*async):\n    pass",
            "def f(**async):\n    pass",
            "def f(*, async=1):\n    pass",
            "x = lambda async: 1",
            "f(async=1)",
            "import async",
            "import a.async",
            "import a as async",
            "from a import async",
            "from a import b as async",
            "try:\n    pass\nexcept E as async:\n    pass",
        ] {
            for code in [code.to_owned(), code.replace("async", "await")] {
                let tree = tree(&code, NAMES);
                assert!(!tree.contains("ASYNC") && !tree.contains("AWAIT"), "{code}: {tree}");
            }
        }
    }

    #[test]
    fn a_real_uppercase_name_keeps_its_spelling() {
        let tree = tree("ASYNC = async\nx = a.AWAIT\nf(ASYNC=await)", None);
        assert_eq!(tree.matches("\"ASYNC\"").count(), 2, "{tree}");
        assert_eq!(tree.matches("\"AWAIT\"").count(), 1, "{tree}");
        assert_eq!(tree.matches("\"async\"").count(), 1, "{tree}");
        assert_eq!(tree.matches("\"await\"").count(), 1, "{tree}");
    }

    #[test]
    fn a_normalized_name_stays_as_the_parser_reads_it() {
        let tree = tree("\u{FF21}SYNC = async", NAMES);
        assert!(tree.contains("\"ASYNC\""), "{tree}");
    }

    #[test]
    fn an_async_def_stays_async() {
        let module = parse("async def f():\n    await a\n", NAMES).unwrap();
        let Stmt::FunctionDef(def) = &module.body[0] else {
            panic!("{module:?}");
        };
        assert!(def.is_async);
        assert!(matches!(&def.body[0], Stmt::Expr(s) if s.value.is_await_expr()));
    }

    #[test]
    fn a_coroutine_keeps_its_loops_and_contexts_on_an_unknown_series() {
        let code = "async def f():\n    async for a in b:\n        await a\n    async with c as d:\n        await d\n";
        let module = parse(code, None).unwrap();
        let Stmt::FunctionDef(def) = &module.body[0] else {
            panic!("{module:?}");
        };
        assert!(def.is_async);
        assert!(matches!(&def.body[0], Stmt::For(s) if s.is_async));
        assert!(matches!(&def.body[1], Stmt::With(s) if s.is_async));
    }

    #[test]
    fn text_without_a_name_to_rename_parses_as_it_is() {
        for code in ["# async", "x = 'await'", "async def f():\n    pass"] {
            assert_eq!(names_async(code, NAMES), code);
            parse(code, NAMES).unwrap();
        }
    }

    #[test]
    fn both_words_are_reserved_from_the_reserving_series() {
        assert!(parse("async = 1", Some(ASYNC_RESERVED)).is_err());
        assert!(parse("async = 1", NAMES).is_ok());
    }
}
