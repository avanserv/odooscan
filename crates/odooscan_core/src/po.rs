//! Translation catalogs: the `.po` files of `i18n/` and `i18n_extra/` and their `.pot` templates,
//! read as Odoo reads them with polib, and the rules on their entries.

use rustc_hash::FxHashMap;

use crate::facts::{PoFacts, Pos, RawDiag};
use crate::placeholders::{self, Fields, Specs};
use crate::suppress::Suppressions;

/// A message of a catalog. The header, whose msgid is empty, is left out, as polib reads it apart.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Entry {
    pub msgctxt: Option<String>,
    pub msgid: String,
    /// None for a plural entry, whose `msgstr[n]` forms Odoo reads as an empty translation.
    pub msgstr: Option<String>,
    /// The line of `msgid`.
    pub line: u32,
    /// The first and last lines of the entry, its comments included.
    pub lines: (u32, u32),
    /// The text of each `#.` comment, such as `module: sale` or `odoo-python`.
    pub extracted: Vec<String>,
    /// Each reference of the `#:` comments, such as `code:addons/sale/models/sale_order.py:0`.
    pub references: Vec<String>,
    pub fuzzy: bool,
    /// Written with `#~`, which Odoo skips unless a template revives it.
    pub obsolete: bool,
}

impl Entry {
    /// What polib matches an entry of a template by: its msgid, after its msgctxt when it has one.
    fn key(&self) -> String {
        match self.msgctxt.as_deref() {
            Some(context) if !context.is_empty() => format!("{context}\u{4}{}", self.msgid),
            _ => self.msgid.clone(),
        }
    }
}

/// The string a continuation line adds to.
#[derive(Clone, Copy, Default)]
enum Field {
    #[default]
    Other,
    Context,
    Id,
    Str,
}

/// The entry being read, until another `msgid` or a comment after its msgstr closes it, as polib
/// closes it. A continuation line leaves polib in the state of its keyword, so a comment after a
/// continued msgid stays in the entry.
#[derive(Default)]
struct Partial {
    entry: Entry,
    has_id: bool,
    has_str: bool,
    plural: bool,
    obsolete: bool,
    field: Field,
}

/// The entries of the catalog `text`, obsolete ones included.
pub(crate) fn entries(text: &str) -> Vec<Entry> {
    let mut out = vec![];
    let mut partial = Partial::default();
    for (i, line) in text.trim_start_matches('\u{feff}').lines().enumerate() {
        partial.read(line.trim(), i as u32 + 1, &mut out);
    }
    partial.finish(&mut out);
    out
}

impl Partial {
    fn read(&mut self, line: &str, number: u32, out: &mut Vec<Entry>) {
        let (line, obsolete) = obsolete_line(line).map_or((line, false), |rest| (rest, true));
        if obsolete && self.has_id && !self.obsolete {
            self.finish(out);
        }
        // polib skips a blank line, so comments above one stay with the msgid below it.
        if line.is_empty() {
            return;
        }
        self.strings(line, number, out);
        self.obsolete |= obsolete;
        if self.entry.lines.0 == 0 {
            self.entry.lines.0 = number;
        }
        self.entry.lines.1 = number;
    }

    /// Reads a comment, a continuation or a keyword line.
    fn strings(&mut self, line: &str, number: u32, out: &mut Vec<Entry>) {
        if let Some(rest) = line.strip_prefix('#') {
            if self.has_id && self.has_str {
                self.finish(out);
            }
            self.comment(rest);
        } else if line.starts_with('"') {
            self.append(&unquote(line));
        } else {
            let (keyword, rest) = line.split_at(line.find([' ', '\t', '"']).unwrap_or(line.len()));
            self.keyword(keyword, unquote(rest), number, out);
        }
    }

    fn keyword(&mut self, keyword: &str, value: String, number: u32, out: &mut Vec<Entry>) {
        if matches!(keyword, "msgctxt" | "msgid") && self.has_id {
            self.finish(out);
        }
        self.field = Field::Other;
        match keyword {
            "msgctxt" => {
                self.entry.msgctxt = Some(value);
                self.field = Field::Context;
            }
            "msgid" => {
                self.entry.msgid = value;
                self.entry.line = number;
                self.has_id = true;
                self.field = Field::Id;
            }
            "msgstr" => {
                self.entry.msgstr = Some(value);
                self.has_str = true;
                self.field = Field::Str;
            }
            "msgid_plural" => self.plural = true,
            _ if keyword.starts_with("msgstr[") => {
                self.plural = true;
                self.has_str = true;
            }
            _ => {}
        }
    }

    fn comment(&mut self, rest: &str) {
        let Some(kind) = rest.chars().next() else {
            return;
        };
        let text = &rest[kind.len_utf8()..];
        match kind {
            '.' => self.entry.extracted.push(text.trim().to_owned()),
            ':' => self.entry.references.extend(text.split_whitespace().map(str::to_owned)),
            ',' => self.entry.fuzzy |= text.split(',').any(|flag| flag.trim() == "fuzzy"),
            _ => {}
        }
    }

    fn append(&mut self, value: &str) {
        match self.field {
            Field::Context => self.entry.msgctxt.get_or_insert_default().push_str(value),
            Field::Id => self.entry.msgid.push_str(value),
            Field::Str => self.entry.msgstr.get_or_insert_default().push_str(value),
            Field::Other => {}
        }
    }

    fn finish(&mut self, out: &mut Vec<Entry>) {
        let partial = std::mem::take(self);
        let mut entry = partial.entry;
        if partial.plural {
            entry.msgstr = None;
        }
        entry.obsolete = partial.obsolete;
        let complete = partial.plural || entry.msgstr.is_some();
        if partial.has_id && complete && !entry.msgid.is_empty() {
            out.push(entry);
        }
    }
}

/// What follows the `#~` of a line of an obsolete entry. polib strips one `#~` before a space and
/// reads the rest as any other line.
fn obsolete_line(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("#~")?;
    (rest.starts_with(char::is_whitespace) && !rest.trim().is_empty()).then(|| rest.trim())
}

/// The string `text` opens with, its escapes read as polib reads them: `\n`, `\t`, `\r`, `\"` and
/// `\\`, any other backslash being kept.
fn unquote(text: &str) -> String {
    let Some(inner) = text.trim_start().strip_prefix('"') else {
        return String::new();
    };
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => break,
            '\\' => match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some(e @ ('"' | '\\')) => out.push(e),
                Some(e) => out.extend(['\\', e]),
                None => out.push('\\'),
            },
            c => out.push(c),
        }
    }
    out
}

/// The `<module>.pot` beside a `.po` file, which Odoo merges into it with polib before loading it
/// (18.0 `odoo/tools/translate.py:800-826`, 20.0 `:1381-1407`). A `.po` entry the template lacks
/// turns obsolete and one it holds takes its obsolete mark, a `#~` entry being revived.
pub(crate) struct Template(FxHashMap<String, bool>);

impl Template {
    pub(crate) fn parse(text: &str) -> Self {
        // The last entry of a message wins, as polib merges the template's entries in order.
        Self(entries(text).into_iter().map(|e| (e.key(), e.obsolete)).collect())
    }

    fn loads(&self, entry: &Entry) -> bool {
        self.0.get(&entry.key()) == Some(&false)
    }
}

/// The findings of the `.po` catalog `text`, merged with its `template` when its folder has one, and
/// its suppressions. A suppression comment anywhere among an entry's comments covers the entry, as
/// msgmerge and translation tools move a translator comment to the top.
pub(crate) fn analyze(text: &str, template: Option<&Template>) -> (PoFacts, Suppressions) {
    let all = entries(text);
    let entries: Vec<&Entry> = all
        .iter()
        .filter(|e| template.map_or(!e.obsolete, |t| t.loads(e)))
        .collect();
    let mut diags: Vec<RawDiag> = entries.iter().filter_map(|e| placeholder_mismatch(e)).collect();
    let fuzzy = entries
        .iter()
        .filter(|e| e.fuzzy && e.msgstr.as_deref().is_some_and(|s| !s.is_empty()));
    diags.extend(fuzzy.map(|e| {
        RawDiag::new(
            "OD802",
            pos(e),
            "the translation is marked fuzzy, but Odoo loads it as a reviewed one; check it and remove the flag",
        )
    }));
    diags.extend(untranslated(&entries));
    let suppressions = Suppressions::parse(text).moved(|line| {
        let at = all.partition_point(|e| e.lines.1 < line);
        all.get(at).filter(|e| e.lines.0 <= line).map_or(line, |e| e.line)
    });
    (PoFacts { diags }, suppressions)
}

fn pos(entry: &Entry) -> Pos {
    Pos {
        line: entry.line,
        col: 1,
    }
}

/// OD803, once per catalog at its first message without a translation.
fn untranslated(entries: &[&Entry]) -> Option<RawDiag> {
    let (mut first, mut empty, mut messages) = (None, 0, 0);
    for entry in entries {
        match entry.msgstr.as_deref() {
            Some("") => {
                first = first.or(Some(*entry));
                empty += 1;
            }
            Some(_) => {}
            None => continue,
        }
        messages += 1;
    }
    Some(RawDiag::new(
        "OD803",
        pos(first?),
        format!("{empty} of {messages} messages are untranslated, so Odoo shows them in the source language"),
    ))
}

/// OD801 on an entry of code Odoo loads: one with a translation, a `code:` reference and the
/// comment that tells the Python ones from those of the web client (18.0
/// `odoo/tools/translate.py:857-871`, `:1835-1843`).
fn placeholder_mismatch(entry: &Entry) -> Option<RawDiag> {
    let msgstr = entry.msgstr.as_deref().filter(|s| !s.is_empty())?;
    if !entry.references.iter().any(|r| r.starts_with("code:")) {
        return None;
    }
    let commented = |what: &str| entry.extracted.iter().any(|c| c.contains(what));
    let message = if commented("odoo-python") {
        python_mismatch(&entry.msgid, msgstr)
    } else if commented("odoo-javascript") {
        sprintf_mismatch(&placeholders::sprintf(&entry.msgid), msgstr)
    } else {
        None
    }?;
    Some(RawDiag::new("OD801", pos(entry), message))
}

/// How the translation `msgstr` of a Python message fails its placeholders. A message is read as
/// `%` formatting when one of its `%` precedes something other than a space, so that `10% off` is
/// text, and a placeholder takes an argument: `_()` formats nothing without arguments, so a message
/// with `%%` alone is not. It is read otherwise as `str.format()` when it has `{}` fields and only
/// plain ones.
fn python_mismatch(msgid: &str, msgstr: &str) -> Option<String> {
    let percent = msgid.match_indices('%').any(|(i, _)| !msgid[i + 1..].starts_with(' '));
    let takes = |s: &Specs| percent && (s.positional > 0 || !s.keys.is_empty());
    if let Some(source) = placeholders::printf(msgid).ok().filter(takes) {
        return printf_mismatch(&source, msgstr);
    }
    let source = placeholders::braces(msgid)
        .ok()
        .filter(|f| f.simple && (f.auto > 0 || !f.names.is_empty()))?;
    braces_mismatch(&source, msgstr)
}

const SOURCE_SHOWN: &str = "so _() shows the source text instead";

fn printf_mismatch(source: &Specs, msgstr: &str) -> Option<String> {
    let target = match placeholders::printf(msgstr) {
        Ok(target) => target,
        Err(spec) => {
            return Some(format!(
                "Python cannot read the placeholder '{spec}' of the translation, {SOURCE_SHOWN}"
            ));
        }
    };
    if source.positional == 0 && target.positional == 1 && target.keys.is_empty() && takes_any_object(msgstr) {
        return Some(
            "the translation takes 1 argument by position where the message takes keys, so it shows the arguments as one mapping"
                .to_owned(),
        );
    }
    if target.positional != source.positional {
        return Some(format!(
            "the translation takes {} by position where the message takes {}, {SOURCE_SHOWN}",
            arguments(target.positional),
            source.positional
        ));
    }
    keys_mismatch(&source.keys, &target.keys, |k| format!("%({k})s"), SOURCE_SHOWN)
}

fn braces_mismatch(source: &Fields, msgstr: &str) -> Option<String> {
    let target = match placeholders::braces(msgstr) {
        Ok(target) => target,
        Err(field) => {
            return Some(format!(
                "str.format() cannot read '{field}' in the translation, so formatting it raises"
            ));
        }
    };
    let numbered = |f: &Fields| f.names.iter().any(|n| placeholders::is_number(n));
    if target.auto != source.auto && !numbered(source) && !numbered(&target) {
        return Some(format!(
            "the translation has {} '{{}}' fields where the message has {}, so formatting it raises or leaves out a value",
            target.auto, source.auto
        ));
    }
    keys_mismatch(
        &arguments_taken(source),
        &arguments_taken(&target),
        |k| format!("{{{k}}}"),
        "so formatting it raises or shows another value",
    )
}

/// Whether `%` fills the one placeholder of `msgstr` with any object, such as the mapping of a
/// message whose placeholders take keys, rather than raising as `%d` does.
fn takes_any_object(msgstr: &str) -> bool {
    let unescaped = msgstr.replace("%%", "");
    unescaped.split_once('%').is_some_and(|(_, spec)| {
        let conversion = spec.chars().find(|c| c.is_ascii_alphabetic() && !"hlL".contains(*c));
        conversion.is_some_and(|c| "rsa".contains(c))
    })
}

/// The names and numbers of the arguments `fields` take, a `{}` field taking the next number, as
/// `str.format()` fills `{}` and `{0}` alike.
fn arguments_taken(fields: &Fields) -> Vec<String> {
    let auto = (0..fields.auto).map(|n| n.to_string());
    fields.names.iter().cloned().chain(auto).collect()
}

/// How the translation `msgstr` of a web client message fails its placeholders. `_t()` formats
/// nothing without arguments, which a message without placeholders is not given.
fn sprintf_mismatch(source: &Specs, msgstr: &str) -> Option<String> {
    if source.positional == 0 && source.keys.is_empty() {
        return None;
    }
    let target = placeholders::sprintf(msgstr);
    if target.positional != source.positional {
        let effect = if target.positional > source.positional {
            "so the web client cannot fill the extra ones"
        } else {
            "so the web client leaves out a value"
        };
        return Some(format!(
            "the translation has {} '%s' where the message has {}, {effect}",
            target.positional, source.positional
        ));
    }
    keys_mismatch(
        &source.keys,
        &target.keys,
        |k| format!("%({k})s"),
        "so the web client fills it with undefined or nothing",
    )
}

/// A key of the translation the message lacks, which formatting cannot fill, or else a key of the
/// message the translation leaves out. Their order does not matter.
fn keys_mismatch(source: &[String], target: &[String], show: impl Fn(&str) -> String, effect: &str) -> Option<String> {
    if let Some(extra) = target.iter().find(|k| !source.contains(k)) {
        return Some(format!(
            "the translation's placeholder '{}' is not in the message, {effect}",
            show(extra)
        ));
    }
    let missing = source.iter().find(|k| !target.contains(k))?;
    Some(format!(
        "the translation leaves out the placeholder '{}' of the message, so its value never shows",
        show(missing)
    ))
}

fn arguments(n: usize) -> String {
    if n == 1 {
        "1 argument".to_owned()
    } else {
        format!("{n} arguments")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One entry of Python code, as a catalog of `kiosk` writes it.
    fn python(msgid: &str, msgstr: &str) -> String {
        format!(
            "#. module: kiosk\n#. odoo-python\n#: code:addons/kiosk/models/stand.py:0\nmsgid \"{msgid}\"\nmsgstr \"{msgstr}\"\n"
        )
    }

    fn javascript(msgid: &str, msgstr: &str) -> String {
        format!(
            "#. module: kiosk\n#. odoo-javascript\n#: code:addons/kiosk/static/src/stand.js:0\nmsgid \"{msgid}\"\nmsgstr \"{msgstr}\"\n"
        )
    }

    fn findings(text: &str) -> Vec<(&'static str, u32, String)> {
        analyze(text, None)
            .0
            .diags
            .into_iter()
            .map(|d| (d.code, d.pos.line, d.message))
            .collect()
    }

    fn od801(text: &str) -> Option<String> {
        findings(text).into_iter().find(|f| f.0 == "OD801").map(|f| f.2)
    }

    #[test]
    fn an_entry_takes_its_comments_strings_and_line() {
        let text = "# Kiosk translations\nmsgid \"\"\nmsgstr \"\"\n\"Language: fr\\n\"\n\n\
            #. module: kiosk\n#: code:addons/kiosk/a.py:0 model:ir.model,name:kiosk.model_stand\n\
            #, fuzzy, python-format\nmsgid \"\"\n\"Stand \"\n\"%s\"\nmsgstr \"Stand %s\"\n";
        let entry = Entry {
            msgid: "Stand %s".to_owned(),
            msgstr: Some("Stand %s".to_owned()),
            line: 9,
            lines: (6, 12),
            extracted: vec!["module: kiosk".to_owned()],
            references: vec![
                "code:addons/kiosk/a.py:0".to_owned(),
                "model:ir.model,name:kiosk.model_stand".to_owned(),
            ],
            fuzzy: true,
            ..Entry::default()
        };
        assert_eq!(entries(text), [entry]);
    }

    #[test]
    fn a_msgstr_continues_over_lines_and_a_comment_after_msgid_stays_in_the_entry() {
        let text = "msgid \"Stand\"\n# kept by a translator\nmsgstr \"\"\n\"Stand \"\n\"de vente\"\n#. module: kiosk\nmsgid \"Booth\"\nmsgstr \"Cabine\"\n";
        let read: Vec<_> = entries(text)
            .into_iter()
            .map(|e| (e.msgid, e.msgstr, e.lines))
            .collect();
        assert_eq!(
            read,
            [
                ("Stand".to_owned(), Some("Stand de vente".to_owned()), (1, 5)),
                ("Booth".to_owned(), Some("Cabine".to_owned()), (6, 8))
            ]
        );
    }

    #[test]
    fn a_comment_after_a_continued_msgid_stays_in_the_entry() {
        let text = "#. odoo-python\n#: code:a.py:0\nmsgid \"\"\n\"%(a)s\"\n# note\nmsgstr \"%(b)s\"\n";
        assert!(od801(text).is_some_and(|m| m.contains("'%(b)s'")));
    }

    #[test]
    fn a_line_of_obsolete_marks_is_read_without_recursion() {
        let text = format!("{}\nmsgid \"Stand\"\nmsgstr \"Stand\"\n", "#~".repeat(100_000));
        let read: Vec<_> = entries(&text).into_iter().map(|e| e.obsolete).collect();
        assert_eq!(read, [false]);
    }

    #[test]
    fn comments_above_a_blank_line_stay_with_the_msgid_below_it() {
        let text = "#. odoo-python\n#: code:addons/kiosk/a.py:0\n\nmsgid \"%(a)s\"\nmsgstr \"%(b)s\"\n";
        assert!(od801(text).is_some_and(|m| m.contains("'%(b)s'")));
    }

    #[test]
    fn escapes_are_read_as_polib_reads_them() {
        let text = r#"msgid "Say \"hi\"\n\tto \\ \x"
msgstr "a""#;
        assert_eq!(entries(text)[0].msgid, "Say \"hi\"\n\tto \\ \\x");
    }

    #[test]
    fn crlf_a_bom_and_a_context_are_read() {
        let text = "\u{feff}msgctxt \"menu\"\r\nmsgid \"Open\"\r\nmsgstr \"Ouvrir\"\r\nmsgid \"Close\"\r\nmsgstr \"Fermer\"\r\n";
        let read: Vec<_> = entries(text)
            .into_iter()
            .map(|e| (e.msgctxt, e.msgid, e.line))
            .collect();
        assert_eq!(
            read,
            [
                (Some("menu".to_owned()), "Open".to_owned(), 2),
                (None, "Close".to_owned(), 4)
            ]
        );
    }

    #[test]
    fn the_header_is_skipped_obsolete_entries_are_marked_and_plurals_have_no_msgstr() {
        let text = "#, fuzzy\nmsgid \"\"\nmsgstr \"Language: fr\\n\"\n\n\
            # kept by a translator\n#~ msgid \"Gone\"\n#~ msgstr \"Parti\"\n\
            #. module: kiosk\nmsgid \"%s stand\"\nmsgid_plural \"%s stands\"\nmsgstr[0] \"%s stand\"\nmsgstr[1] \"\"\n";
        let read: Vec<_> = entries(text)
            .into_iter()
            .map(|e| (e.msgid, e.msgstr, e.obsolete))
            .collect();
        assert_eq!(
            read,
            [
                ("Gone".to_owned(), Some("Parti".to_owned()), true),
                ("%s stand".to_owned(), None, false)
            ]
        );
    }

    #[test]
    fn keys_that_differ_are_reported() {
        let found = od801(&python("%(count)s orders", "%(nombre)s ventes"));
        assert_eq!(
            found.as_deref(),
            Some(
                "the translation's placeholder '%(nombre)s' is not in the message, so _() shows the source text instead"
            )
        );
        let dropped = od801(&python("%(count)s orders for %(name)s", "ventes de %(name)s"));
        assert!(dropped.is_some_and(|m| m.contains("leaves out the placeholder '%(count)s'")));
        assert_eq!(od801(&python("%(a)s then %(b)s", "%(b)s puis %(a)s")), None);
    }

    #[test]
    fn a_count_or_a_broken_placeholder_is_reported() {
        let count = od801(&python("%s of %s", "%s"));
        assert!(count.is_some_and(|m| m.contains("takes 1 argument by position where the message takes 2")));
        let broken = od801(&python("%s left", "%s restant (%)"));
        assert!(broken.is_some_and(|m| m.contains("'%)'")));
        assert_eq!(od801(&python("%s at 100%%", "%s à 100%%")), None);
    }

    #[test]
    fn a_positional_placeholder_for_keys_shows_the_mapping() {
        let shown = od801(&python("%(count)s left", "%s restants"));
        assert!(shown.is_some_and(|m| m.contains("shows the arguments as one mapping")));
        let raises = od801(&python("%(count)s left", "%d restants"));
        assert!(raises.is_some_and(|m| m.contains("so _() shows the source text instead")));
    }

    #[test]
    fn a_percent_before_a_space_is_text() {
        assert_eq!(od801(&python("10% off", "10 % de remise")), None);
        assert_eq!(od801(&python("Below 100 (got %)", "Sous 100 (reçu %s)")), None);
    }

    #[test]
    fn plain_brace_fields_are_compared_by_name_and_count() {
        let renamed = od801(&python("{name} paid", "{nom} a payé"));
        assert!(renamed.is_some_and(|m| m.contains("'{nom}' is not in the message")));
        let count = od801(&python("{} of {}", "{}"));
        assert!(count.is_some_and(|m| m.contains("1 '{}' fields where the message has 2")));
        let broken = od801(&python("{name} paid", "{name} a payé}"));
        assert!(broken.is_some_and(|m| m.contains("cannot read '}'")));
        assert_eq!(od801(&python("{a} and {b}", "{b} et {a}")), None);
        assert_eq!(od801(&python("{order.name} paid", "{order.nom} payé")), None);
        assert_eq!(od801(&python("Paid", "{Payé}")), None);
        assert_eq!(od801(&python("{} of {}", "{1} de {0}")), None);
        assert_eq!(od801(&python("{0}", "{}")), None);
        let dropped = od801(&python("{} of {}", "{0}"));
        assert!(dropped.is_some_and(|m| m.contains("leaves out the placeholder '{1}'")));
        let percent = od801(&python("{share}%% of {amount}", "{amount} à {part}%"));
        assert!(percent.is_some_and(|m| m.contains("'{part}'")));
    }

    #[test]
    fn javascript_reads_sprintf_placeholders() {
        let key = od801(&javascript("%(count)s seats", "%(places)s places"));
        assert!(key.is_some_and(|m| m.contains("'%(places)s'")));
        let count = od801(&javascript("%s of %s", "%s"));
        assert!(count.is_some_and(|m| m.contains("1 '%s' where the message has 2")));
        assert_eq!(od801(&javascript("{name} seats", "{nom} places")), None);
        assert_eq!(od801(&javascript("100%% of %s", "100%% de %s")), None);
        assert_eq!(od801(&javascript("Swan", "Cygne %s")), None);
    }

    #[test]
    fn only_code_entries_with_a_translation_are_checked() {
        let model = "#. module: kiosk\n#: model:ir.model.fields,field_description:kiosk.field_stand__name\n\
            msgid \"%(count)s stands\"\nmsgstr \"%(nombre)s stands\"\n";
        assert_eq!(od801(model), None);
        assert_eq!(od801(&python("%(count)s orders", "")), None);
        let uncommented = "#: code:addons/kiosk/a.py:0\nmsgid \"%s\"\nmsgstr \"%d %d\"\n";
        assert_eq!(od801(uncommented), None);
    }

    #[test]
    fn fuzzy_entries_with_a_translation_are_reported() {
        let text = "#, fuzzy\nmsgid \"\"\nmsgstr \"Language: fr\\n\"\n\n\
            #, fuzzy\nmsgid \"Stand\"\nmsgstr \"Stand\"\n\n#, fuzzy\nmsgid \"Booth\"\nmsgstr \"\"\n";
        let codes: Vec<_> = findings(text).into_iter().map(|f| (f.0, f.1)).collect();
        assert_eq!(codes, [("OD802", 6), ("OD803", 10)]);
    }

    #[test]
    fn untranslated_messages_are_reported_once_at_the_first() {
        let text = "msgid \"Stand\"\nmsgstr \"Stand\"\n\nmsgid \"Booth\"\nmsgstr \"\"\n\nmsgid \"Desk\"\nmsgstr \"\"\n";
        let found = findings(text);
        assert_eq!(
            found,
            [(
                "OD803",
                4,
                "2 of 3 messages are untranslated, so Odoo shows them in the source language".to_owned()
            )]
        );
    }

    #[test]
    fn with_a_template_only_the_messages_it_holds_live_load() {
        let template = Template::parse(
            "msgid \"Stand\"\nmsgstr \"\"\n\nmsgctxt \"menu\"\nmsgid \"Booth\"\nmsgstr \"\"\n\n\
             #~ msgid \"Desk\"\n#~ msgstr \"\"\n\nmsgid \"Shelf\"\nmsgstr \"\"\n",
        );
        let text = "msgid \"Stand\"\nmsgstr \"\"\n\nmsgid \"Booth\"\nmsgstr \"\"\n\n\
            msgctxt \"menu\"\nmsgid \"Booth\"\nmsgstr \"Cabine\"\n\nmsgid \"Desk\"\nmsgstr \"\"\n\n\
            #~ msgid \"Shelf\"\n#~ msgstr \"\"\n";
        let message = |template| analyze(text, template).0.diags.pop().map(|d| d.message);
        assert!(message(Some(&template)).is_some_and(|m| m.starts_with("2 of 3 ")));
        assert!(message(None).is_some_and(|m| m.starts_with("3 of 4 ")));
    }

    #[test]
    fn a_suppression_anywhere_among_the_comments_covers_the_entry() {
        let text = "# odooscan: ignore[OD802]\n#. module: kiosk\n#, fuzzy\nmsgid \"Stand\"\nmsgstr \"Stand\"\n";
        let (facts, suppressions) = analyze(text, None);
        assert_eq!(facts.diags[0].pos.line, 4);
        assert!(!suppressions.allows(4, "OD802"));
        assert!(suppressions.allows(4, "OD801"));
    }
}
