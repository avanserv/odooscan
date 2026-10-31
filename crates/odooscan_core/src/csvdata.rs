use crate::domain;
use crate::facts::{
    AccessRow, ActionValue, DataFacts, FieldWrite, Pos, RawDiag, RecordFact, RecordWrite, RuleFact, Spanned,
    WrittenValue, XmlDef, XmlRef,
};
use crate::xml::{literal, rule_domain_field};

const PUBLIC_GROUPS: &[&str] = &["base.group_public", "base.group_portal", "group_public", "group_portal"];

/// A data CSV is named after its model; its header names fields, with `:id` or `/id` for references.
pub fn analyze(text: &str, model: &str) -> DataFacts {
    let mut facts = DataFacts {
        is_data: true,
        ..DataFacts::default()
    };
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

/// What a row writes to the fields of its record. A database id or a nested `a/b` column replaces
/// the field with what only a run tells, and an empty cell clears it.
fn row_write(facts: &mut DataFacts, row: &csv::StringRecord, columns: &[(&str, bool)], model: &str, pos: Pos) {
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
        });
    }
    facts.writes.push(RecordWrite {
        id: row_id(row, columns),
        model: model.to_owned(),
        fields,
        nested_in: None,
        csv: true,
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

/// Odoo's import takes these as true, `0`, `false`, `no` and an empty cell as false, and refuses
/// any other word (`ir_fields.py`, `_str_to_boolean`), which aborts the file.
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

/// Whether an access row gives more than read, or None for a file of another model. Odoo checks
/// active rows only, and a file without the column leaves the field at its default, true.
fn writes(row: &csv::StringRecord, columns: &[(&str, bool)], model: &str) -> Option<bool> {
    let set = |name: &str| truthy(cell(row, columns, name));
    let granted = match model {
        "ir.model.access" => set("perm_write") || set("perm_create") || set("perm_unlink"),
        "ir.access" => cell(row, columns, "operation").contains(['c', 'u', 'd']),
        _ => return None,
    };
    Some(granted && (column(columns, "active").is_none() || set("active")))
}

/// An ir.model.access or ir.access row. Only one with a group grants on `ir.access`, where a row
/// without restricts every user.
fn access_row(facts: &mut DataFacts, row: &csv::StringRecord, columns: &[(&str, bool)], model: &str, pos: Pos) {
    let Some(i) = column(columns, "model_id") else { return };
    let Some(write_like) = writes(row, columns, model) else {
        return;
    };
    let target = row.get(i).unwrap_or_default().to_owned();
    let group = cell(row, columns, "group_id");
    if write_like && PUBLIC_GROUPS.contains(&group) {
        facts.diags.push(RawDiag::new(
            "OD402",
            pos,
            format!("{group} gets write, create or unlink on {target}; public and portal access should be read-only and go through controllers"),
        ));
    }
    let no_group = group.is_empty() && database_id(row, columns, "group_id").unwrap_or_default().is_empty();
    if model == "ir.access" && no_group {
        return;
    }
    facts.access.push(AccessRow {
        id: row_id(row, columns),
        model: target,
        by_name: model == "ir.access" && !columns[i].1,
        pos,
        global_write: no_group && write_like,
    });
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
/// record, and an empty cell clears the field.
fn rule_row(facts: &mut DataFacts, row: &csv::StringRecord, columns: &[(&str, bool)], model: &str, pos: Pos) {
    let Some(domain_field) = rule_domain_field(model) else {
        return;
    };
    let cell = |name: &str| Some(row.get(column(columns, name)?).unwrap_or_default());
    let domain = cell(domain_field).map_or(ActionValue::Unset, |text| {
        literal(Some(Spanned {
            value: text.trim().to_owned(),
            pos,
        }))
    });
    let (target, by_name) = rule_model(row, columns, model, pos);
    facts.rules.push(RuleFact {
        id: row_id(row, columns),
        model: target,
        by_name,
        fields: domain::written_fields(&domain),
        domain,
        active: cell("active").map(truthy),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each field of the row as `name=value`, in the order of the columns.
    fn row(header: &str, row: &str) -> Vec<String> {
        let facts = analyze(&format!("{header}\n{row}\n"), "m");
        let shown = |f: &FieldWrite| match &f.value {
            WrittenValue::Ids(ids) => {
                let ids: Vec<&str> = ids.iter().map(|i| i.value.as_str()).collect();
                format!("{}={}", f.name, ids.join("+"))
            }
            WrittenValue::Text(text) => format!("{}='{}'", f.name, text.value),
            WrittenValue::Opaque => format!("{}?", f.name),
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

    fn access(header: &str, row: &str) -> DataFacts {
        analyze(&format!("{header}\n{row}\n"), "ir.model.access")
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
        assert!(access(HEADER, "a,m,base.group_portal,1,0").diags.is_empty());
        assert_eq!(access(HEADER, "a,m,base.group_portal,1,1").diags[0].code, "OD402");
        let on_access = |active: &str| {
            let text = format!("id,model_id,group_id/id,operation,active\na,x.y,base.group_portal,cu,{active}\n");
            analyze(&text, "ir.access").diags.len()
        };
        assert_eq!(on_access("0"), 0);
        assert_eq!(on_access("1"), 1);
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
        assert_eq!(facts.diags[0].code, "OD402");
    }

    #[test]
    fn an_access_row_without_group_grants_nothing() {
        let facts = analyze(
            "id,model_id,group_id/id,operation\na,x.y,,cu\nb,x.y,base.group_user,r\n",
            "ir.access",
        );
        assert_eq!(facts.access.len(), 1);
        assert_eq!(facts.access[0].id.as_deref(), Some("b"));
        assert!(facts.access[0].by_name);
        assert!(!facts.access[0].global_write);
        assert!(facts.diags.is_empty());
    }

    fn rules(text: &str, model: &str) -> Vec<RuleFact> {
        analyze(text, model).rules
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
        assert!(matches!(empty.domain, ActionValue::Opaque));
        assert_eq!(empty.active, Some(false));
        let by_db_id = &rules("id,model_id/.id\nr,3\n", "ir.rule")[0];
        assert!(matches!(by_db_id.model, ActionValue::Opaque));
    }
}
