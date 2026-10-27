use crate::facts::{AccessRow, DataFacts, Pos, RawDiag, RecordFact, Spanned, XmlDef, XmlRef};

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
                value: (*c).to_owned(),
                pos: at(1),
            })
            .collect(),
    });
    let col = |name: &str| columns.iter().position(|(c, _)| *c == name);
    for row in reader.records() {
        let Ok(row) = row else { continue };
        let pos = at(row.position().map_or(1, |p| p.line()));
        if let Some(id) = col("id").and_then(|i| row.get(i)).filter(|v| !v.is_empty()) {
            facts.defs.push(XmlDef {
                id: id.to_owned(),
                model: Some(model.to_owned()),
                pos,
            });
        }
        for (i, (_, is_ref)) in columns.iter().enumerate() {
            if !is_ref {
                continue;
            }
            for id in row
                .get(i)
                .unwrap_or("")
                .split(',')
                .map(str::trim)
                .filter(|v| !v.is_empty())
            {
                facts.refs.push(XmlRef { id: id.to_owned(), pos });
            }
        }
        let Some(i) = col("model_id") else { continue };
        let target = row.get(i).unwrap_or_default().to_owned();
        let group = col("group_id").and_then(|i| row.get(i)).unwrap_or_default();
        let flag = |name: &str| col(name).and_then(|i| row.get(i)).is_some_and(|v| v.trim() == "1");
        let write_like = match model {
            "ir.model.access" => flag("perm_write") || flag("perm_create") || flag("perm_unlink"),
            "ir.access" => col("operation")
                .and_then(|i| row.get(i))
                .is_some_and(|op| op.contains(['c', 'u', 'd'])),
            _ => continue,
        };
        if write_like && PUBLIC_GROUPS.contains(&group) {
            facts.diags.push(RawDiag::new(
                "OD402",
                pos,
                format!("{group} gets write, create or unlink on {target}; public and portal access should be read-only and go through controllers"),
            ));
        }
        let by_name = model == "ir.access" && !columns[i].1;
        facts.access.push(AccessRow { model: target, by_name });
    }
    facts
}
