use ruff_text_size::TextSize;

use crate::domain;
use crate::facts::{
    AccessRow, ActionValue, DataFacts, FieldWrite, Flag, Perms, Pos, RawDiag, RecordFact, RecordWrite, RuleFact,
    RuleGroups, Spanned, WrittenValue, XmlDef, XmlRef,
};
use crate::grants::PUBLIC_GROUPS;
use crate::python::Locator;
use crate::series::Series;
use crate::xml::{PERM_FIELDS, literal, operation_perms, rule_domain_field};

/// The first series that reads a data CSV from an `io.StringIO`, which splits lines at LF alone
/// (`18.0`, 18.0 `odoo/tools/convert.py:683`). Below, from 11.0, a `codecs` reader splits them as
/// `str.splitlines` does (17.0 `odoo/tools/convert.py:700`, `odoo/tools/pycompat.py:7,14`).
const STRING_IO: Series = Series { major: 18, minor: 0 };

/// The most characters Python's `csv` reads into one cell, `csv.field_size_limit()` by default.
const FIELD_LIMIT: usize = 131_072;

/// Whether a CSV cell names a public or portal group, as a full XML id or as one of base's own.
fn public_cell(group: &str) -> bool {
    PUBLIC_GROUPS
        .iter()
        .any(|g| *g == group || g.strip_prefix("base.") == Some(group))
}

/// How the Odoo of a series splits a data CSV it loads into lines for Python's `csv` reader.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lines {
    /// At any boundary `str.splitlines` knows, so a lone CR ends a row.
    Universal,
    /// At LF alone, so a CR outside quotes that does not end a line stops the reader.
    Lf,
}

impl Lines {
    /// The lines of every series the module may target: an unknown one may load on either
    /// reader, the newest first.
    pub fn of(series: Option<Series>) -> &'static [Lines] {
        match series {
            None => &[Lines::Lf, Lines::Universal],
            Some(s) if s >= STRING_IO => &[Lines::Lf],
            Some(_) => &[Lines::Universal],
        }
    }
}

/// The facts of a data CSV the Odoo of a series fails to read as a strict UTF-8 file, with the
/// place it stops at.
pub fn undecoded(pos: Pos) -> DataFacts {
    DataFacts {
        diags: vec![RawDiag::new("OD000", pos, "the file is not UTF-8")],
        unreadable: true,
        ..DataFacts::default()
    }
}

/// A data CSV is named after its model; its header names fields, with `:id` or `/id` for references.
/// `strict` gives the lines of each series a file Odoo loads may target, none for a file it does
/// not load. The import fails only when every one refuses, so a module that may target a series
/// that loads the file is not told it fails; the first gives the place and the reason.
pub fn analyze(text: &str, model: &str, strict: &[Lines]) -> DataFacts {
    let mut facts = DataFacts {
        is_data: true,
        ..DataFacts::default()
    };
    let refusals: Option<Vec<_>> = strict.iter().map(|&lines| refused(text, lines)).collect();
    if let Some((pos, message)) = refusals.and_then(|r| r.into_iter().next()) {
        facts.diags.push(RawDiag::new("OD000", pos, message));
    }
    let mut reader = csv::ReaderBuilder::new().flexible(true).from_reader(text.as_bytes());
    let header = match reader.headers() {
        Ok(h) => h.clone(),
        Err(e) => {
            facts
                .diags
                .push(RawDiag::new("OD000", Pos { line: 1, col: 1 }, e.to_string()));
            return facts;
        }
    };
    let columns: Vec<(&str, bool)> = header
        .iter()
        .map(|c| match c.strip_suffix(":id").or_else(|| c.strip_suffix("/id")) {
            Some(field) => (field, true),
            None => (c, false),
        })
        .collect();
    let at = |line: u64| Pos {
        line: line as u32,
        col: 1,
    };
    facts.records.push(RecordFact {
        model: Spanned {
            value: model.to_owned(),
            pos: at(1),
        },
        fields: columns
            .iter()
            .filter(|(c, _)| *c != "id")
            .map(|(c, _)| Spanned {
                value: database_field(c).unwrap_or(c).to_owned(),
                pos: at(1),
            })
            .collect(),
        csv: true,
    });
    for row in reader.records() {
        let Ok(row) = row else { continue };
        let pos = at(row.position().map_or(1, |p| p.line()));
        row_ids(&mut facts, &row, &columns, model, pos);
        row_write(&mut facts, &row, &columns, model, pos);
        access_row(&mut facts, &row, &columns, model, pos);
        rule_row(&mut facts, &row, &columns, model, pos);
    }
    facts
}

fn column(columns: &[(&str, bool)], name: &str) -> Option<usize> {
    columns.iter().position(|(c, _)| *c == name)
}

fn row_ids(facts: &mut DataFacts, row: &csv::StringRecord, columns: &[(&str, bool)], model: &str, pos: Pos) {
    if let Some(id) = column(columns, "id").and_then(|i| row.get(i)).filter(|v| !v.is_empty()) {
        facts.defs.push(XmlDef {
            id: id.to_owned(),
            model: Some(model.to_owned()),
            pos,
            themed: false,
            assigned: false,
            no_create: false,
        });
    }
    for (i, (_, is_ref)) in columns.iter().enumerate() {
        if !is_ref {
            continue;
        }
        for id in ref_ids(row.get(i).unwrap_or("")) {
            facts.refs.push(XmlRef { id: id.to_owned(), pos });
        }
    }
}

/// Whether a column holds a field of the rows nested in a field of the record.
fn nested_column(column: &str) -> bool {
    database_field(column).is_none() && column.contains('/')
}

/// What a row writes to the fields of its record. A database id or a nested `a/b` column replaces
/// the field with what only a run tells, and an empty cell clears it. A row with values only in
/// nested columns continues the record above, and Odoo skips a row of empty cells.
fn row_write(facts: &mut DataFacts, row: &csv::StringRecord, columns: &[(&str, bool)], model: &str, pos: Pos) {
    let mut cells = columns.iter().zip(row.iter());
    if !cells.any(|((column, _), cell)| !nested_column(column) && !cell.is_empty()) {
        return;
    }
    let mut fields = vec![];
    for (i, (column, is_ref)) in columns.iter().enumerate().filter(|(_, (c, _))| *c != "id") {
        let spanned = |value: &str| Spanned {
            value: value.to_owned(),
            pos,
        };
        let name = database_field(column).or_else(|| column.split_once('/').map(|(field, _)| field));
        let cell = row.get(i).unwrap_or_default();
        let value = if name.is_some() || cell.is_empty() {
            WrittenValue::Opaque
        } else if *is_ref {
            WrittenValue::Ids(ref_ids(cell).map(spanned).collect())
        } else {
            WrittenValue::Text(spanned(cell))
        };
        fields.push(FieldWrite {
            name: name.unwrap_or(column).to_owned(),
            value,
            incremental: false,
        });
    }
    facts.writes.push(RecordWrite {
        id: row_id(row, columns),
        model: model.to_owned(),
        pos,
        fields,
        nested_in: None,
        csv: true,
        context_defaults: vec![],
        no_create: false,
    });
}

/// The ids of a `:id` cell.
fn ref_ids(cell: &str) -> impl Iterator<Item = &str> {
    cell.split(',').map(str::trim).filter(|v| !v.is_empty())
}

/// The field of a `field/.id` or `field.id` column, which holds a database id.
fn database_field(column: &str) -> Option<&str> {
    let name = column.strip_suffix(".id")?;
    Some(name.strip_suffix('/').unwrap_or(name)).filter(|n| !n.is_empty())
}

fn database_id<'r>(row: &'r csv::StringRecord, columns: &[(&str, bool)], field: &str) -> Option<&'r str> {
    row.get(columns.iter().position(|(c, _)| database_field(c) == Some(field))?)
}

/// Odoo's import takes these as true, `0`, `false`, `no` and an empty cell as false, with their
/// translations in the installed languages (`ir_fields.py`, `_str_to_boolean`). From saas~14.3
/// it refuses any other word, which aborts the file. Below, that word loads as true with an
/// `ImportWarning`, which `truthy` reads as false.
fn truthy(cell: &str) -> bool {
    ["1", "true", "yes"].iter().any(|t| cell.trim().eq_ignore_ascii_case(t))
}

fn row_id(row: &csv::StringRecord, columns: &[(&str, bool)]) -> Option<String> {
    let id = column(columns, "id").and_then(|i| row.get(i));
    id.filter(|v| !v.is_empty()).map(str::to_owned)
}

fn cell<'r>(row: &'r csv::StringRecord, columns: &[(&str, bool)], name: &str) -> &'r str {
    column(columns, name).and_then(|i| row.get(i)).unwrap_or_default()
}

/// What an access row grants, or None for a file of another model. Odoo checks active rows only,
/// and a file without the column leaves the field at its default, true.
fn grants(row: &csv::StringRecord, columns: &[(&str, bool)], model: &str) -> Option<Perms> {
    let set = |name: &str| truthy(cell(row, columns, name));
    let [read, write, create, unlink] = match model {
        "ir.model.access" => PERM_FIELDS.map(set),
        "ir.access" => operation_perms(cell(row, columns, "operation")).map(|f| f == Flag::Set(true)),
        _ => return None,
    };
    let perms = Perms {
        read,
        write,
        create,
        unlink,
    };
    let active = column(columns, "active").is_none() || set("active");
    Some(if active { perms } else { Perms::default() })
}

/// An ir.model.access or ir.access row. Only one with a group grants on `ir.access`, where a row
/// without restricts every user.
fn access_row(facts: &mut DataFacts, row: &csv::StringRecord, columns: &[(&str, bool)], model: &str, pos: Pos) {
    let Some(i) = column(columns, "model_id") else { return };
    let Some(grants) = grants(row, columns, model) else {
        return;
    };
    let target = row.get(i).unwrap_or_default().to_owned();
    let group = cell(row, columns, "group_id");
    let by_db_id = !database_id(row, columns, "group_id").unwrap_or_default().is_empty();
    let no_group = group.is_empty() && !by_db_id;
    if model == "ir.access" && no_group {
        return;
    }
    let write_like = grants.writes();
    facts.access.push(AccessRow {
        id: row_id(row, columns),
        model: target,
        by_name: !columns[i].1,
        pos,
        group: (!no_group).then(|| group.to_owned()),
        grants,
        global_write: no_group && write_like,
        public_write: (write_like && public_cell(group)).then(|| group.to_owned()),
        model_access: model == "ir.model.access",
        domain: access_domain(row, columns, model),
    });
}

/// The domain of an `ir.access` row, which limits its grant to the records it holds.
fn access_domain(row: &csv::StringRecord, columns: &[(&str, bool)], model: &str) -> Option<String> {
    if model != "ir.access" {
        return None;
    }
    let text = cell(row, columns, rule_domain_field(model)?).trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// What a row gives `model_id`. An `ir.access` CSV names the model, where any other column of a name
/// goes through a name search only a run can answer.
fn rule_model(row: &csv::StringRecord, columns: &[(&str, bool)], model: &str, pos: Pos) -> (ActionValue, bool) {
    let Some(i) = column(columns, "model_id") else {
        let written = if database_id(row, columns, "model_id").is_some() {
            ActionValue::Opaque
        } else {
            ActionValue::Unset
        };
        return (written, false);
    };
    let by_name = !columns[i].1;
    if by_name && model != "ir.access" {
        return (ActionValue::Opaque, false);
    }
    let value = row.get(i).unwrap_or_default().trim().to_owned();
    (literal(Some(Spanned { value, pos })), by_name)
}

/// A row of an `ir.rule` or `ir.access` file. A column it lacks is left to the other writes of the
/// record, and an empty cell clears the field, which leaves an empty domain. The import stores a
/// blank cell as it is, which the domain check refuses (18.0 `odoo/addons/base/models/ir_fields.py:123`).
fn rule_row(facts: &mut DataFacts, row: &csv::StringRecord, columns: &[(&str, bool)], model: &str, pos: Pos) {
    let Some(domain_field) = rule_domain_field(model) else {
        return;
    };
    let cell = |name: &str| Some(row.get(column(columns, name)?).unwrap_or_default());
    let domain = cell(domain_field).map_or(ActionValue::Unset, |text| {
        let value = text.trim().to_owned();
        let spanned = Spanned { value, pos };
        if text.is_empty() {
            ActionValue::Literal(spanned)
        } else {
            literal(Some(spanned))
        }
    });
    let access = model == "ir.access";
    let perms = if access {
        cell("operation").map_or([Flag::Unset; 4], operation_perms)
    } else {
        PERM_FIELDS.map(|name| cell(name).map_or(Flag::Unset, |c| Flag::Set(truthy(c))))
    };
    let (target, by_name) = rule_model(row, columns, model, pos);
    facts.rules.push(RuleFact {
        id: row_id(row, columns),
        pos,
        access,
        model: target,
        by_name,
        fields: domain::written_fields(&domain),
        domain,
        active: cell("active").map(truthy),
        groups: rule_groups(row, columns, if access { "group_id" } else { "groups" }),
        perms,
    });
}

/// What a row gives the groups of a rule. The import replaces a many2many with the ids of its cell.
fn rule_groups(row: &csv::StringRecord, columns: &[(&str, bool)], field: &str) -> RuleGroups {
    let replaced = |added| RuleGroups {
        added,
        keeps: false,
        opaque: false,
    };
    if let Some(i) = columns.iter().position(|&(c, is_ref)| c == field && is_ref) {
        return replaced(ref_ids(row.get(i).unwrap_or_default()).map(str::to_owned).collect());
    }
    if database_id(row, columns, field).is_some() || column(columns, field).is_some() {
        return RuleGroups {
            added: vec![],
            keeps: false,
            opaque: true,
        };
    }
    RuleGroups::default()
}

/// Where Odoo's import of the CSV `text` fails, and why. It reads every row before it loads one,
/// so a cell the reader refuses beats a row the import cannot take.
fn refused(text: &str, lines: Lines) -> Option<(Pos, String)> {
    if text.is_empty() {
        let message = "the file is empty, and Odoo's import reads its first row as the header";
        return Some((Pos { line: 1, col: 1 }, message.to_owned()));
    }
    let loc = Locator::new(text);
    let at = |offset: usize| loc.pos(TextSize::try_from(offset).unwrap_or_default());
    let (header, rows) = match Reader::default().read(text, lines) {
        Ok(read) => read,
        Err((offset, message)) => return Some((at(offset), message.to_owned())),
    };
    let relational: Vec<(usize, &String)> = header
        .iter()
        .enumerate()
        .filter(|(_, c)| relational_column(c))
        .collect();
    let last = relational.last()?.0;
    // The import drops a row of empty cells, and reads every relational column of each other one.
    let row = rows.iter().find(|r| r.filled && r.cells <= last)?;
    let (_, column) = relational.iter().find(|(i, _)| row.cells <= *i)?;
    let short = format!("the row ends before column '{column}', where Odoo's import reads a relational field");
    Some(match row.open_quote {
        Some(quote) => (at(quote), format!("this quote runs to the end of the file, so {short}")),
        None => (at(row.start), short),
    })
}

/// Whether Odoo's import reads `column` as a path into a relational field: a reference, a database
/// id or a subfield (18.0 `odoo/models.py:166`, `:1508`). A plain column may name one too, which
/// only the field's type tells.
fn relational_column(column: &str) -> bool {
    match column.strip_suffix(":id").or_else(|| column.strip_suffix("/id")) {
        Some(field) => !field.is_empty() && !field.starts_with('/'),
        None => database_field(column).is_some() || (nested_column(column) && !column.starts_with('/')),
    }
}

/// The lines of `text`, each with its end and by its offset, as Odoo hands them to the reader.
fn split_lines(text: &str, lines: Lines) -> Vec<(usize, &str)> {
    let mut split = vec![];
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let crlf = lines == Lines::Universal && c == '\r' && chars.next_if(|&(_, next)| next == '\n').is_some();
        if crlf || line_end(c, lines) {
            let end = i + c.len_utf8() + usize::from(crlf);
            split.push((start, &text[start..end]));
            start = end;
        }
    }
    if start < text.len() {
        split.push((start, &text[start..]));
    }
    split
}

fn line_end(c: char, lines: Lines) -> bool {
    match lines {
        Lines::Lf => c == '\n',
        Lines::Universal => matches!(
            c,
            '\n' | '\r' | '\x0B' | '\x0C' | '\x1C' | '\x1D' | '\x1E' | '\u{85}' | '\u{2028}' | '\u{2029}'
        ),
    }
}

/// A record of Python's `csv` reader: where it starts, how many cells it has, whether one holds
/// anything, and the quote it leaves open at the end of the file.
#[derive(Debug, Default)]
struct Record {
    start: usize,
    cells: usize,
    filled: bool,
    open_quote: Option<usize>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    #[default]
    StartRecord,
    StartField,
    InField,
    InQuoted,
    QuoteInQuoted,
    EatNewline,
}

/// Where the reader stops, and why.
type Stop = (usize, &'static str);

/// Python's `csv` reader as Odoo builds it: a comma, a double quote, no escape character, not
/// strict. Each state is CPython's (`Modules/_csv.c`, `parse_process_char`, 3.10 to 3.14).
#[derive(Default)]
struct Reader {
    state: State,
    record: Record,
    /// The characters of the cell being read.
    cell: usize,
    cell_start: usize,
    quote: usize,
    cr: usize,
    /// The header, the first record, once read; until then the cells and the text of its cell.
    header: Option<Vec<String>>,
    cells: Vec<String>,
    text: String,
    rows: Vec<Record>,
}

impl Reader {
    /// The header and the records below it.
    fn read(mut self, text: &str, lines: Lines) -> Result<(Vec<String>, Vec<Record>), Stop> {
        for (start, line) in split_lines(text, lines) {
            for (i, c) in line.char_indices() {
                self.char(Some(c), start + i)?;
            }
            self.char(None, start + line.len())?;
            if self.state == State::StartRecord {
                self.end_record();
            }
        }
        // At the end of the file the reader keeps the quoted cell it is still in.
        if self.state == State::InQuoted {
            self.record.open_quote = Some(self.quote);
            self.save();
            self.end_record();
        }
        Ok((self.header.unwrap_or_default(), self.rows))
    }

    /// Reads `c` at `at`, none for the end of a line.
    fn char(&mut self, c: Option<char>, at: usize) -> Result<(), Stop> {
        if c == Some('\r') && self.state != State::InQuoted {
            self.cr = at;
        }
        match self.state {
            State::StartRecord => self.start_record(c, at),
            State::StartField => self.start_field(c, at),
            State::InField => self.in_field(c),
            State::InQuoted => self.in_quoted(c),
            State::QuoteInQuoted => self.after_quote(c),
            State::EatNewline => self.eat_newline(c),
        }
    }

    fn start_record(&mut self, c: Option<char>, at: usize) -> Result<(), Stop> {
        match c {
            None => Ok(()),
            Some('\n' | '\r') => {
                self.state = State::EatNewline;
                Ok(())
            }
            Some(_) => {
                self.record.start = at;
                self.start_field(c, at)
            }
        }
    }

    fn start_field(&mut self, c: Option<char>, at: usize) -> Result<(), Stop> {
        self.cell_start = at;
        if self.delimits(c) {
            return Ok(());
        }
        match c {
            Some('"') => {
                self.quote = at;
                self.state = State::InQuoted;
                Ok(())
            }
            Some(c) => {
                self.state = State::InField;
                self.add(c)
            }
            None => Ok(()),
        }
    }

    fn in_field(&mut self, c: Option<char>) -> Result<(), Stop> {
        if self.delimits(c) {
            return Ok(());
        }
        c.map_or(Ok(()), |c| self.add(c))
    }

    fn in_quoted(&mut self, c: Option<char>) -> Result<(), Stop> {
        match c {
            None => Ok(()),
            Some('"') => {
                self.state = State::QuoteInQuoted;
                Ok(())
            }
            Some(c) => self.add(c),
        }
    }

    /// After a quote inside quotes: a second one is a quote in the cell, anything else but a
    /// delimiter goes on as an unquoted cell.
    fn after_quote(&mut self, c: Option<char>) -> Result<(), Stop> {
        if c == Some('"') {
            self.state = State::InQuoted;
            return self.add('"');
        }
        if self.delimits(c) {
            return Ok(());
        }
        self.state = State::InField;
        c.map_or(Ok(()), |c| self.add(c))
    }

    fn eat_newline(&mut self, c: Option<char>) -> Result<(), Stop> {
        match c {
            None => self.state = State::StartRecord,
            Some('\n' | '\r') => {}
            Some(_) => {
                return Err((
                    self.cr,
                    "a carriage return outside quotes that ends no line, which Python's csv reader refuses",
                ));
            }
        }
        Ok(())
    }

    /// Ends the cell at a comma, a line break or the end of the line, which every state but quotes
    /// and line breaks does alike.
    fn delimits(&mut self, c: Option<char>) -> bool {
        let next = match c {
            None => State::StartRecord,
            Some('\n' | '\r') => State::EatNewline,
            Some(',') => State::StartField,
            Some(_) => return false,
        };
        self.save();
        self.state = next;
        true
    }

    fn add(&mut self, c: char) -> Result<(), Stop> {
        if self.cell >= FIELD_LIMIT {
            return Err((
                self.cell_start,
                "a cell of more than 131072 characters, which Python's csv reader refuses",
            ));
        }
        self.cell += 1;
        if self.header.is_none() {
            self.text.push(c);
        }
        Ok(())
    }

    fn save(&mut self) {
        self.record.cells += 1;
        self.record.filled |= self.cell > 0;
        self.cell = 0;
        if self.header.is_none() {
            let cell = std::mem::take(&mut self.text);
            self.cells.push(cell);
        }
    }

    fn end_record(&mut self) {
        let record = std::mem::take(&mut self.record);
        match self.header {
            None => self.header = Some(std::mem::take(&mut self.cells)),
            Some(_) => self.rows.push(record),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each field of the row as `name=value`, in the order of the columns.
    fn row(header: &str, row: &str) -> Vec<String> {
        let facts = analyze(&format!("{header}\n{row}\n"), "m", &[]);
        let shown = |f: &FieldWrite| match &f.value {
            WrittenValue::Ids(ids) => {
                let ids: Vec<&str> = ids.iter().map(|i| i.value.as_str()).collect();
                format!("{}={}", f.name, ids.join("+"))
            }
            WrittenValue::Text(text) => format!("{}='{}'", f.name, text.value),
            WrittenValue::Model(_) | WrittenValue::Opaque => format!("{}?", f.name),
        };
        facts.writes[0].fields.iter().map(shown).collect()
    }

    #[test]
    fn writes_ids_and_text_by_column() {
        assert_eq!(row("id,a:id,b/id,c", "r,x, y , z "), ["a=x", "b=y", "c=' z '"]);
    }

    #[test]
    fn a_cell_of_several_ids_splits_at_commas() {
        assert_eq!(row("id,a:id", "r,\"x, y,,z\""), ["a=x+y+z"]);
    }

    #[test]
    fn replaces_a_field_by_what_only_a_run_tells() {
        assert_eq!(row("id,a/.id,b.id", "r,3,4"), ["a?", "b?"]);
        assert_eq!(row("id,a/b:id,a/c/id,c/d", "r,x,y,z"), ["a?", "a?", "c?"]);
        assert_eq!(row("id,a,b:id", "r,,"), ["a?", "b?"]);
    }

    #[test]
    fn a_row_with_nothing_of_its_own_writes_nothing() {
        assert!(analyze("id,name\n,\n", "m", &[]).writes.is_empty());
        let facts = analyze("id,lines/name\nr,x\n,y\n", "m", &[]);
        assert_eq!(facts.writes.len(), 1);
    }

    fn access(header: &str, row: &str) -> DataFacts {
        analyze(&format!("{header}\n{row}\n"), "ir.model.access", &[])
    }

    const PERMS: &str = "id,model_id:id,group_id:id,perm_read,perm_write,perm_create,perm_unlink";

    #[test]
    fn a_row_with_no_group_writes_for_every_user() {
        let global = |row: &str| access(PERMS, row).access[0].global_write;
        assert!(global("a,m,,1,1,0,0"));
        assert!(global("a,m,,1,0,True,0"));
        assert!(global("a,m,,1,0,0,YES"));
        assert!(!global("a,m,,1,0,0,0"));
        assert!(!global("a,m,,1,no,false,"));
        assert!(!global("a,m,g,1,1,1,1"));
        let no_column = access("id,model_id:id,perm_write", "a,m,1");
        assert!(no_column.access[0].global_write);
        let by_db_id = |header: &str| access(header, "a,m,3,1").access[0].global_write;
        assert!(!by_db_id("id,model_id:id,group_id/.id,perm_write"));
        assert!(!by_db_id("id,model_id:id,group_id.id,perm_write"));
    }

    #[test]
    fn an_inactive_row_writes_for_nobody() {
        const HEADER: &str = "id,model_id:id,group_id:id,perm_write,active";
        let global = |row: &str| access(HEADER, row).access[0].global_write;
        for off in ["0", "False", "no", ""] {
            assert!(!global(&format!("a,m,,1,{off}")), "active {off:?}");
        }
        for on in ["1", "True", "YES"] {
            assert!(global(&format!("a,m,,1,{on}")), "active {on:?}");
        }
        let public = |row: &str| access(HEADER, row).access[0].public_write.clone();
        assert_eq!(public("a,m,base.group_portal,1,0"), None);
        assert_eq!(
            public("a,m,base.group_portal,1,1").as_deref(),
            Some("base.group_portal")
        );
        let on_access = |active: &str| {
            let text = format!("id,model_id,group_id/id,operation,active\na,x.y,base.group_portal,cu,{active}\n");
            analyze(&text, "ir.access", &[]).access[0].public_write.clone()
        };
        assert_eq!(on_access("0"), None);
        assert_eq!(on_access("1").as_deref(), Some("base.group_portal"));
    }

    #[test]
    fn an_access_row_keeps_its_trimmed_domain() {
        let domain = |header: &str, row: &str| {
            let text = format!("{header}\n{row}\n");
            analyze(&text, "ir.access", &[]).access[0].domain.clone()
        };
        let header = "id,model_id,group_id/id,operation,domain";
        assert_eq!(
            domain(header, "a,x.y,base.group_portal,c,\" [('public', '=', False)] \"").as_deref(),
            Some("[('public', '=', False)]")
        );
        assert_eq!(domain(header, "a,x.y,base.group_portal,c,\"  \""), None);
        assert_eq!(domain(header, "a,x.y,base.group_portal,c,"), None);
        assert_eq!(
            domain("id,model_id,group_id/id,operation", "a,x.y,base.group_portal,c"),
            None
        );
        assert_eq!(access(PERMS, "a,m,base.group_portal,1,1,1,1").access[0].domain, None);
    }

    #[test]
    fn a_database_id_column_names_its_field() {
        let facts = access("id,group_id/.id,model_id.id", "a,3,4");
        let fields: Vec<&str> = facts.records[0].fields.iter().map(|f| f.value.as_str()).collect();
        assert_eq!(fields, ["group_id", "model_id"]);
    }

    #[test]
    fn perm_words_reach_public_rows_too() {
        let facts = access(PERMS, "a,m,base.group_portal,1,True,0,0");
        assert_eq!(facts.access[0].public_write.as_deref(), Some("base.group_portal"));
    }

    #[test]
    fn an_access_row_without_group_grants_nothing() {
        let facts = analyze(
            "id,model_id,group_id/id,operation\na,x.y,,cu\nb,x.y,base.group_user,r\n",
            "ir.access",
            &[],
        );
        assert_eq!(facts.access.len(), 1);
        assert_eq!(facts.access[0].id.as_deref(), Some("b"));
        assert!(facts.access[0].by_name);
        assert!(!facts.access[0].global_write);
        assert!(facts.diags.is_empty());
    }

    #[test]
    fn a_plain_model_column_names_the_model_of_an_access_row() {
        let text = "id,model_id,group_id:id,perm_read,perm_write\na,x.y,base.group_portal,1,1\n";
        assert!(analyze(text, "ir.model.access", &[]).access[0].by_name);
        assert!(!access(PERMS, "a,m,,1,1,0,0").access[0].by_name);
    }

    fn rules(text: &str, model: &str) -> Vec<RuleFact> {
        analyze(text, model, &[]).rules
    }

    #[test]
    fn reads_a_rule_row() {
        let text = "id,model_id:id,domain_force,active\nr,model_a,\"[('a.b', '=', 1)]\",1\n";
        let rule = &rules(text, "ir.rule")[0];
        assert_eq!(rule.id.as_deref(), Some("r"));
        assert!(matches!(&rule.model, ActionValue::Literal(m) if m.value == "model_a"));
        assert!(!rule.by_name);
        assert_eq!(rule.fields[0].value, "a.b");
        assert_eq!(rule.fields[0].pos, Pos { line: 2, col: 1 });
        assert_eq!(rule.active, Some(true));
    }

    #[test]
    fn a_plain_model_column_names_the_model_on_ir_access_only() {
        let text = "id,model_id,domain\nr,a.b,\"[]\"\n";
        let access = &rules(text, "ir.access")[0];
        assert!(access.by_name);
        assert!(matches!(&access.model, ActionValue::Literal(m) if m.value == "a.b"));
        let rule = &rules("id,model_id,domain_force\nr,a.b,[]\n", "ir.rule")[0];
        assert!(matches!(rule.model, ActionValue::Opaque));
    }

    #[test]
    fn a_missing_column_writes_nothing_and_an_empty_cell_clears() {
        let unset = &rules("id,name\nr,n\n", "ir.rule")[0];
        assert!(matches!(unset.model, ActionValue::Unset));
        assert!(matches!(unset.domain, ActionValue::Unset));
        assert_eq!(unset.active, None);
        let empty = &rules("id,model_id:id,domain_force,active\nr,,,\n", "ir.rule")[0];
        assert!(matches!(empty.model, ActionValue::Opaque));
        assert!(matches!(&empty.domain, ActionValue::Literal(d) if d.value.is_empty()));
        assert!(empty.fields.is_empty());
        let blank = &rules("id,domain_force\nr,\"  \"\n", "ir.rule")[0];
        assert!(matches!(blank.domain, ActionValue::Opaque));
        let text_false = &rules("id,domain_force\nr,False\n", "ir.rule")[0];
        assert!(matches!(&text_false.domain, ActionValue::Literal(d) if d.value == "False"));
        assert_eq!(empty.active, Some(false));
        let by_db_id = &rules("id,model_id/.id\nr,3\n", "ir.rule")[0];
        assert!(matches!(by_db_id.model, ActionValue::Opaque));
    }

    #[test]
    fn an_access_row_grants_what_its_columns_say_once_active() {
        let grants = |row: &str| access(&format!("{PERMS},active"), row).access[0].grants;
        let perms = |read, write, create, unlink| Perms {
            read,
            write,
            create,
            unlink,
        };
        assert_eq!(grants("a,m,g,1,0,yes,0,1"), perms(true, false, true, false));
        assert_eq!(grants("a,m,g,1,1,1,1,0"), Perms::default());
        assert_eq!(
            access(PERMS, "a,m,g,0,0,0,1").access[0].grants,
            perms(false, false, false, true)
        );
        let on_access = |operation: &str| {
            let text = format!("id,model_id,group_id/id,operation\na,x.y,base.group_portal,{operation}\n");
            analyze(&text, "ir.access", &[]).access[0].grants
        };
        assert_eq!(on_access("r"), perms(true, false, false, false));
        assert_eq!(on_access("cud"), perms(false, true, true, true));
    }

    /// OD401 counts every row of a model, so one that grants nothing still makes a row.
    #[test]
    fn a_row_that_grants_nothing_is_still_a_row() {
        let rows = access(&format!("{PERMS},active"), "a,m,g,0,0,0,0,1\nb,m,,1,1,1,1,0");
        assert_eq!(rows.access.len(), 2);
        assert!(rows.access.iter().all(|r| r.grants == Perms::default()));
    }

    #[test]
    fn an_access_row_names_its_group() {
        let group = |header: &str, row: &str| access(header, row).access[0].group.clone();
        assert_eq!(
            group(PERMS, "a,m,base.group_portal,1,0,0,0").as_deref(),
            Some("base.group_portal")
        );
        assert_eq!(group(PERMS, "a,m,,1,0,0,0"), None);
        assert_eq!(group("id,model_id:id,perm_read", "a,m,1"), None);
        let by_db_id = "id,model_id:id,group_id/.id,perm_read";
        assert_eq!(group(by_db_id, "a,m,3,1").as_deref(), Some(""));
    }

    #[test]
    fn a_rule_row_reads_its_groups_and_permissions() {
        let rule = |header: &str, row: &str| rules(&format!("{header}\n{row}\n"), "ir.rule").remove(0);
        let read = rule("id,groups/id,perm_read,perm_write", "r,\"base.group_portal, g\",0,1");
        assert_eq!(read.groups.added, ["base.group_portal", "g"]);
        assert!(!read.groups.keeps && !read.groups.opaque && !read.access);
        assert_eq!(
            read.perms,
            [Flag::Set(false), Flag::Set(true), Flag::Unset, Flag::Unset]
        );
        let unset = rule("id,name", "r,n");
        assert!(unset.groups.added.is_empty() && unset.groups.keeps && !unset.groups.opaque);
        assert_eq!(unset.perms, [Flag::Unset; 4]);
        assert!(rule("id,groups/.id", "r,3").groups.opaque);
        assert!(rule("id,groups", "r,Portal").groups.opaque);
        let cleared = rule("id,groups:id", "r,");
        assert!(cleared.groups.added.is_empty() && !cleared.groups.keeps);
        let access = &rules(
            "id,model_id,group_id:id,operation\nr,a.b,base.group_portal,ru\n",
            "ir.access",
        )[0];
        assert!(access.access);
        assert_eq!(access.groups.added, ["base.group_portal"]);
        let set = Flag::Set;
        assert_eq!(access.perms, [set(true), set(true), set(false), set(false)]);
    }

    /// Where and why the import of `text` fails on 18.0, as `(line, col, message)`.
    fn import(text: &str) -> Option<(u32, u32, String)> {
        let lf = analyze(text, "res.partner", &[Lines::Lf]);
        let universal = analyze(text, "res.partner", &[Lines::Universal]);
        let lf = lf.diags.into_iter().next().map(|d| (d.pos.line, d.pos.col, d.message));
        if !text.contains('\r') {
            assert_eq!(
                lf,
                universal
                    .diags
                    .into_iter()
                    .next()
                    .map(|d| (d.pos.line, d.pos.col, d.message))
            );
        }
        lf
    }

    fn failing_line(text: &str) -> Option<u32> {
        import(text).map(|(line, _, _)| line)
    }

    #[test]
    fn a_row_that_ends_before_a_relational_column_fails() {
        let access = "id,name,model_id:id,group_id:id,perm_read\na,A,m,g,1\nb,B,m\nc,C\n";
        let (line, col, message) = import(access).unwrap();
        assert_eq!((line, col), (3, 1));
        assert_eq!(
            message,
            "the row ends before column 'group_id:id', where Odoo's import reads a relational field"
        );
        for header in ["a/id", "a.id", "a/.id", "a/b", "a/b:id"] {
            assert_eq!(failing_line(&format!("id,{header}\nr\n")), Some(2), "{header}");
        }
        for header in ["name", "id", ".id", "/id", ":id", "/b"] {
            assert_eq!(failing_line(&format!("id,{header}\nr\n")), None, "{header}");
        }
    }

    #[test]
    fn a_row_long_enough_or_empty_loads() {
        assert_eq!(failing_line("id,parent_id:id\nr,p,extra,more\n"), None);
        assert_eq!(failing_line("id,parent_id:id,name,comment\nr,p\n"), None);
        assert_eq!(failing_line("id,parent_id:id\nr,\n"), None);
        assert_eq!(failing_line("id,name,parent_id:id\n\n,\n,,\n"), None);
        assert_eq!(failing_line("\nid,parent_id:id\nr\n"), None);
        assert_eq!(failing_line("id,parent_id:id\n\"r\nr\",p\n"), None);
        assert_eq!(failing_line("id,parent_id:id\n\"r\"\"\",p\n"), None);
    }

    #[test]
    fn an_unclosed_quote_fails_where_it_shortens_the_last_row() {
        let (line, col, message) = import("id,name,parent_id:id\na,A,p\nb,\"B,p\nc,C,p\n").unwrap();
        assert_eq!((line, col), (3, 3));
        assert!(
            message.starts_with("this quote runs to the end of the file, so the row ends before column 'parent_id:id'")
        );
        assert_eq!(import("id,parent_id:id,name\nb,p,\"B\nc,C,p\n"), None);
    }

    #[test]
    fn a_lone_cr_fails_from_18() {
        let text = "id,name\nr,a\rb\n";
        let first = analyze(text, "m", &[Lines::Lf]).diags.remove(0);
        assert_eq!((first.pos.line, first.pos.col), (2, 4));
        assert!(first.message.starts_with("a carriage return outside quotes"));
        assert!(analyze(text, "m", &[Lines::Universal]).diags.is_empty());
        assert!(analyze(text, "m", Lines::of(None)).diags.is_empty());
        for quiet in [
            "id,name\r\nr,a\r\n",
            "id,name\nr,\"a\rb\"\n",
            "id,name\nr,a\r\r\n",
            "id,name\nr,a\r",
        ] {
            assert!(analyze(quiet, "m", &[Lines::Lf]).diags.is_empty(), "{quiet:?}");
        }
        assert!(!analyze("id,name\nr,\"a\"\rb\n", "m", &[Lines::Lf]).diags.is_empty());
    }

    #[test]
    fn universal_lines_split_a_row_where_lf_lines_do_not() {
        let text = "id,name,parent_id:id\nr,a\x0Cb,p\n";
        assert_eq!(analyze(text, "m", &[Lines::Lf]).diags.len(), 0);
        let universal = analyze(text, "m", &[Lines::Universal]).diags;
        assert_eq!(universal.len(), 1);
        assert_eq!(universal[0].pos.line, 2);
        assert!(analyze(text, "m", Lines::of(None)).diags.is_empty());
    }

    #[test]
    fn an_unknown_series_reports_what_every_reader_refuses() {
        let reported = |text: &str| {
            let diags = analyze(text, "m", Lines::of(None)).diags;
            diags
                .into_iter()
                .map(|d| (d.pos.line, d.pos.col, d.message))
                .collect::<Vec<_>>()
        };
        let short = reported("id,name,parent_id:id\nr,a\n");
        assert_eq!(short.len(), 1);
        assert_eq!((short[0].0, short[0].1), (2, 1));
        assert!(short[0].2.starts_with("the row ends before column 'parent_id:id'"));
        // The 17.0 reader stops at the short row on line 2, the 18.0 one at the CR.
        let cr = reported("id,parent_id:id\nr\ns,a\rb\n");
        assert_eq!(cr.len(), 1);
        assert_eq!((cr[0].0, cr[0].1), (3, 4));
        assert!(cr[0].2.starts_with("a carriage return outside quotes"));
    }

    #[test]
    fn an_empty_file_or_an_oversized_cell_fails() {
        assert_eq!(
            import(""),
            Some((
                1,
                1,
                "the file is empty, and Odoo's import reads its first row as the header".to_owned()
            ))
        );
        let limit = "x".repeat(FIELD_LIMIT);
        assert_eq!(failing_line(&format!("id,name\nr,{limit}\n")), None);
        let (line, col, message) = import(&format!("id,name\nr,\"{limit}x\"\n")).unwrap();
        assert_eq!((line, col), (2, 3));
        assert!(message.starts_with("a cell of more than 131072 characters"));
    }

    #[test]
    fn a_cell_the_reader_refuses_beats_a_short_row_above_it() {
        assert_eq!(failing_line("id,parent_id:id\nr\ns,a\rb\n"), Some(3));
    }

    #[test]
    fn a_file_not_loaded_is_not_read_as_the_import_does() {
        assert!(analyze("", "m", &[]).diags.is_empty());
        assert!(analyze("id,parent_id:id\nr\n", "m", &[]).diags.is_empty());
    }

    #[test]
    fn lines_follow_the_reader_of_the_series() {
        assert_eq!(Lines::of(Series::parse("17.0")), [Lines::Universal]);
        assert_eq!(Lines::of(Series::parse("saas~17.4")), [Lines::Universal]);
        assert_eq!(Lines::of(Series::parse("18.0")), [Lines::Lf]);
        assert_eq!(Lines::of(None), [Lines::Lf, Lines::Universal]);
    }
}
