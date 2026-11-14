use std::ops::Range;
use std::sync::LazyLock;

use regex::{Captures, Regex};
use roxmltree::{Document, Node, ParsingOptions};

use crate::domain;
use crate::facts::{
    AccessRow, ActionFact, ActionValue, ActionViewId, ArchButton, ArchDomain, ArchElement, ArchEmptied, ArchField,
    ArchModifier, ArchSubview, ArchValued, DataFacts, DomainFact, FieldWrite, Flag, FunctionCall, Lines, Perms, Pos,
    RawDiag, RecordFact, RecordWrite, ReportFact, RuleFact, RuleGroups, ServerFact, Spanned, ViewFact, WrittenValue,
    XmlDef, XmlRef,
};
use crate::fix::{Edit, Fix};
use crate::manifest::{AssetEntry, Directive};
use crate::modifier;
use crate::record_eval;
use crate::series::Series;
use crate::server_code;

/// A `False` second argument makes `ref()` return nothing for an unknown id instead of failing.
static EVAL_REF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\bref\(\s*['"]([\w.]+)['"]\s*(,\s*(?:raise_if_not_found\s*=\s*)?False\b)?"#).unwrap()
});
/// An `eval` that is one `ref()`, and so gives the record of that id.
static WHOLE_EVAL_REF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^\s*ref\(\s*['"]([\w.]+)['"]\s*\)\s*$"#).unwrap());
/// The ids `ir.model.data._update_xmlids` gives to records that Python expressions select.
static UPDATE_XMLID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"['"]xml_id['"]\s*:\s*['"]([\w.]+)['"]"#).unwrap());
/// A key of a context that gives a field its value on create.
static CONTEXT_DEFAULT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bdefault_(\w+)").unwrap());
/// The key of a context that makes Odoo take a view for a type by its id.
static CONTEXT_VIEW_REF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\w+)_view_ref\b").unwrap());
/// A field step followed by more steps, as in `//field[@name='line_ids']//field[...]`, reaches into
/// the subview of line_ids.
static XPATH_CONTAINER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"field\[@name=['"](\w+)['"]\]/+([\w*.-]+)(::)?"#).unwrap());
/// An xpath ending on a field step locates that field wherever it is, subviews included.
static XPATH_LAST_FIELD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"field\[@name=['"](\w+)['"]\]$"#).unwrap());
/// An xpath ending on the step right under a field locates a subview of that field.
static XPATH_LAST_SUBVIEW: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"field\[@name=['"]\w+['"]\]/+([\w-]+)(\[[^\]]*\])*$"#).unwrap());
/// A step on a view tag other than through a field may land in the main view or in any subview.
static XPATH_AMBIGUOUS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(^|/)(list|tree|kanban|column)\b").unwrap());
/// An xpath that names `tree` as a tag, in a step or an axis, rather than in an attribute or a string.
static XPATH_TREE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(^|[^\w.@'"-])tree($|[^\w.'"-])"#).unwrap());
/// A call to `name()` or `local-name()`. Beside the string `tree`, it may test an element's name in
/// any form, as `contains(local-name(), 'tree')` or `name(.) != 'tree'`, so the pair guards.
static XPATH_NAME_CALL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(local-)?name\s*\(").unwrap());
/// The quoted string `tree`, in either quote.
static XPATH_TREE_STRING: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"'tree'|"tree""#).unwrap());

/// Path segment for an arch position whose model cannot be told without the parent view.
pub const UNKNOWN_CONTEXT: &str = "?";
/// Prefix of a path segment naming a `<field position="...">` locator rather than a subview.
pub const LOCATOR: char = '~';
/// An xpath step that names its element, as in `field[@name='x']` or `t[@t-call="x"]`.
static XPATH_NAMED_STEP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"([A-Za-z][\w-]*)\[@(name|id|t-call)=['"]([^'"]+)['"]\]"#).unwrap());
/// An xpath ending on a script step that names its file, the script a spec places others against.
static XPATH_SCRIPT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"script\[@src=(?:'([^']*)'|"([^"]*)")\]$"#).unwrap());
/// Attributes by which a spec names the element it locates.
const LOCATING_ATTRS: [&str; 3] = ["name", "id", "t-call"];
static ACTION_REF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"%\(([\w.]+)\)d").unwrap());

/// Output attributes an `<attribute>` spec may set, over which a renamed `t-out` would win.
const OUT_BESIDE_ESCAPE: [&str; 2] = ["t-esc", "t-field"];

/// Attributes that make the value of a `<field>` something other than its `eval` or its text.
const INDIRECT_VALUE: [&str; 4] = ["ref", "search", "file", "type"];

/// Tags under which a `<field>` opens a subview on the field's comodel.
const SUBVIEWS: &[&str] = &["list", "tree", "form", "kanban", "graph", "pivot", "calendar", "search"];

/// Attributes of a calendar whose value starts with a field the view loads.
const CALENDAR_FIELD_ATTRS: [&str; 5] = ["date_start", "date_delay", "date_stop", "color", "all_day"];

/// Attributes Odoo reads as Python expressions on the records of an arch.
const MODIFIERS: [&str; 4] = ["column_invisible", "invisible", "readonly", "required"];

/// Tags a spec may locate in the main view or in the subview of any x2many field.
const AMBIGUOUS_SPECS: &[&str] = &["list", "tree", "kanban", "column"];

const CRON: &str = "ir.cron";

/// What every `report_type` that renders a QWeb template starts with.
const QWEB_REPORT: &str = "qweb-";

/// What opens a CDATA section, which a text node's range starts on.
const CDATA: &str = "<![CDATA[";
/// What closes a CDATA section.
const CDATA_END: &str = "]]>";
/// The entities XML defines itself, each of one character. Any other comes from a DOCTYPE.
const PREDEFINED_ENTITIES: [&str; 5] = ["lt", "gt", "amp", "quot", "apos"];

/// The view type of a `<template>`.
pub const QWEB: &str = "qweb";

/// Arch roots that name no view type: QWeb's own, and those of inheriting specs.
const UNTYPED_ROOTS: &[&str] = &["t", "data", "xpath"];

/// Models whose `key` names a view. A `website.page` shares its key with the view it delegates to.
const KEYED_MODELS: &[&str] = &[
    "ir.ui.view",
    "theme.ir.ui.view",
    "website.page",
    "website.controller.page",
];

/// The facts of a data XML libxml2 stops reading at `pos`, as it is not UTF-8 and names no other
/// encoding.
pub fn undecoded(pos: Pos) -> DataFacts {
    let message = "the file is not UTF-8, and its declaration names no other encoding";
    DataFacts {
        diags: vec![RawDiag::new("OD000", pos, message)],
        unreadable: true,
        ..DataFacts::default()
    }
}

pub fn analyze(text: &str, series: Option<Series>) -> DataFacts {
    let lines = Lines::new(text);
    let mut facts = DataFacts::default();
    if text.trim().is_empty() {
        return facts;
    }
    let opts = ParsingOptions {
        allow_dtd: true,
        ..ParsingOptions::default()
    };
    let doc = match Document::parse_with_options(text, opts) {
        Ok(doc) => doc,
        Err(e) => {
            facts.unreadable = true;
            let p = e.pos();
            facts.diags.push(RawDiag::new(
                "OD000",
                Pos {
                    line: p.row,
                    col: p.col,
                },
                e.to_string(),
            ));
            return facts;
        }
    };
    let root = doc.root_element();
    // Anything else, such as <templates> under static/, is an asset bundle template, not data.
    if matches!(root.tag_name().name(), "odoo" | "openerp" | "data") {
        facts.is_data = true;
        Walker {
            lines: &lines,
            series,
            facts: &mut facts,
            around: vec![],
        }
        .data(root);
    }
    facts
}

/// The XML id an element defines. Only `<template>` reads `t-name`, and `el.get('id', ...)` lets an
/// `id` win even when empty.
fn defined_id<'a>(node: Node<'a, '_>) -> Option<&'a str> {
    node.attribute("id")
        .or_else(|| node.attribute("t-name").filter(|_| node.has_tag_name("template")))
}

struct Walker<'a> {
    lines: &'a Lines,
    series: Option<Series>,
    facts: &'a mut DataFacts,
    /// The subviews around the arch position being walked, as `ArchModifier::subviews` says.
    around: Vec<ArchSubview>,
}

impl Walker<'_> {
    fn pos(&self, node: Node) -> Pos {
        self.lines.pos(node.range().start)
    }

    fn def(&mut self, node: Node, model: Option<&str>) {
        if let Some(id) = defined_id(node) {
            let pos = self.pos(node);
            let tag = node.tag_name().name();
            self.facts.defs.push(XmlDef {
                id: id.to_owned(),
                model: model.map(str::to_owned),
                pos,
                themed: matches!(tag, "template" | "asset"),
                assigned: false,
                no_create: matches!(tag, "record" | "template") && no_create(node),
            });
        }
    }

    fn reference(&mut self, id: &str, node: Node) {
        let pos = self.pos(node);
        self.facts.refs.push(XmlRef { id: id.to_owned(), pos });
    }

    fn groups(&mut self, node: Node) {
        if let Some(groups) = node.attribute("groups") {
            for g in groups
                .split(',')
                .map(|g| g.trim().trim_start_matches(['!', '-']))
                .filter(|g| !g.is_empty())
            {
                self.reference(g, node);
            }
        }
    }

    /// `ref()` calls inside eval attributes, anywhere under `node`.
    fn eval_refs(&mut self, node: Node) {
        for n in node.descendants().filter(Node::is_element) {
            if let Some(eval) = n.attribute("eval") {
                // `ref('module.' + name)` builds the id at runtime; the literal part is not an id.
                for cap in EVAL_REF
                    .captures_iter(eval)
                    .filter(|c| !c[1].ends_with('.') && c.get(2).is_none())
                {
                    self.reference(&cap[1], n);
                }
            }
        }
    }

    fn update_xmlids(&mut self, node: Node) {
        if node.attribute("model") != Some("ir.model.data") || node.attribute("name") != Some("_update_xmlids") {
            return;
        }
        let pos = self.pos(node);
        for eval in node.descendants().filter_map(|n| n.attribute("eval")) {
            for cap in UPDATE_XMLID.captures_iter(eval) {
                self.facts.defs.push(XmlDef {
                    id: cap[1].to_owned(),
                    model: None,
                    pos,
                    themed: false,
                    assigned: true,
                    no_create: false,
                });
            }
        }
    }

    fn function_call(&mut self, node: Node) {
        if let (Some(model), Some(method)) = (node.attribute("model"), attr_value(node, "name", self.lines)) {
            self.facts.functions.push(FunctionCall {
                model: model.to_owned(),
                method,
                default_of: default_of(node),
            });
        }
    }

    fn unsafe_qweb(&mut self, node: Node) {
        for n in node.descendants().filter(|n| n.has_attribute("t-raw")) {
            let pos = self.pos(n);
            self.facts.diags.push(RawDiag::new(
                "OD307",
                pos,
                "t-raw renders unescaped HTML; use t-out and wrap trusted HTML in Markup",
            ));
        }
    }

    fn data(&mut self, node: Node) {
        for child in node.children().filter(Node::is_element) {
            match child.tag_name().name() {
                "data" | "odoo" | "openerp" => self.data(child),
                "record" => self.record(child),
                "template" => {
                    self.def(child, Some("ir.ui.view"));
                    // Odoo reads an empty `inherit_id` as no parent.
                    if let Some(parent) = child.attribute("inherit_id").filter(|p| !p.is_empty()) {
                        self.reference(parent, child);
                    }
                    self.template(child);
                    self.groups(child);
                    self.unsafe_qweb(child);
                }
                "menuitem" => self.menuitem(child),
                "act_window" | "report" | "asset" => self.shorthand(child),
                "function" => {
                    self.update_xmlids(child);
                    self.eval_refs(child);
                    self.function_call(child);
                }
                _ => {}
            }
        }
    }

    /// The tags that stand for a record, with the id and the groups of one.
    fn shorthand(&mut self, node: Node) {
        let model = match node.tag_name().name() {
            "act_window" => "ir.actions.act_window",
            "report" => "ir.actions.report",
            _ => "ir.asset",
        };
        self.def(node, Some(model));
        self.groups(node);
        match node.tag_name().name() {
            "act_window" => self.act_window(node),
            "report" => self.report_tag(node),
            _ => {}
        }
    }

    /// A `<template>` is a QWeb view whose arch is its content.
    fn template(&mut self, node: Node) {
        let mut view = ViewFact {
            id: defined_id(node).map(str::to_owned),
            inherit: node.attribute("inherit_id").map(str::to_owned),
            has_arch: true,
            kind: Some(QWEB.to_owned()),
            ..ViewFact::default()
        };
        self.facts.view_keys.extend(node.attribute("key").map(str::to_owned));
        // The loader turns the `<template>` into the arch's root, which keeps its `t-call`, unless
        // it extends a parent: a truthy `inherit_id` renames the root to `<data>`, which only
        // lends its children to the inheritance, `primary` included.
        let extends = node.attribute("inherit_id").is_some_and(|p| !p.is_empty());
        view.primary = Some(!extends || node.attribute("primary") == Some("True"));
        let body = node.descendants().skip(usize::from(extends));
        view.calls.extend(body.filter_map(|n| t_call(n, self.lines)));
        if extends {
            extension_assets(node, &mut view.assets);
        } else {
            view.assets = match node.attribute("t-call") {
                Some(_) => asset_of(node).into_iter().collect(),
                None => rendered(node).into_iter().filter_map(asset_of).collect(),
            };
        }
        // That root is a `<t>` in the arch, and a spec can locate it by its call.
        if let Some(value) = node.attribute("t-call").filter(|_| !extends) {
            view.elements.push(ArchElement {
                tag: "t".to_owned(),
                attr: "t-call".to_owned(),
                value: value.to_owned(),
            });
        }
        for n in node.descendants().skip(1).filter(Node::is_element) {
            let pos = self.pos(n);
            arch_element(n, pos, &mut view);
            if n.has_attribute("t-esc") && !locates(n) {
                view.escapes.push(escape(n, pos));
            }
        }
        self.facts.views.push(view);
    }

    fn menuitem(&mut self, node: Node) {
        self.def(node, Some("ir.ui.menu"));
        for attr in ["parent", "action"] {
            if let Some(id) = node.attribute(attr) {
                self.reference(id, node);
            }
        }
        self.groups(node);
        for child in node.children().filter(|c| c.has_tag_name("menuitem")) {
            self.menuitem(child);
        }
    }

    fn record(&mut self, node: Node) {
        self.def(node, node.attribute("model"));
        let Some(model) = node.attribute("model") else { return };
        self.record_write(node, model);
        let mut record = RecordFact {
            model: Spanned {
                value: model.to_owned(),
                pos: self.pos(node),
            },
            fields: vec![],
            csv: false,
        };
        let mut view = ViewFact {
            id: node.attribute("id").map(str::to_owned),
            ..ViewFact::default()
        };
        for field in node.children().filter(|c| c.has_tag_name("field")) {
            // Records nested in a one2many field are created with their own XML ids.
            for nested in field.children().filter(|c| c.has_tag_name("record")) {
                self.record(nested);
            }
            let Some(name) = field.attribute("name") else { continue };
            let pos = self.pos(field);
            record.fields.push(Spanned {
                value: name.to_owned(),
                pos,
            });
            self.record_field(field, name, model, &mut view);
        }
        if model == "ir.ui.view" {
            view.primary = primary_view(node, self.lines);
            view.unread_parent = view.inherit.is_none() && last_field_of(node, "inherit_id").is_some_and(holds_record);
            self.facts.views.push(view);
        }
        self.model_facts(node, model);
        self.facts.records.push(record);
    }

    /// Odoo creates a record before those nested in it, so this write comes first.
    fn record_write(&mut self, node: Node, model: &str) {
        let mut fields: Vec<FieldWrite> = vec![];
        for field in node.children().filter(|c| c.has_tag_name("field")) {
            let Some(name) = field.attribute("name") else { continue };
            fields.retain(|f| f.name != name);
            fields.push(FieldWrite {
                name: name.to_owned(),
                value: written(field, self.lines),
                incremental: incremental(field),
            });
        }
        let enclosing = node.parent_element().filter(|p| p.has_tag_name("field"));
        let nested_in = enclosing.and_then(|f| {
            let parent = f.parent_element()?.attribute("model")?;
            Some((parent.to_owned(), f.attribute("name")?.to_owned()))
        });
        self.facts.writes.push(RecordWrite {
            id: node.attribute("id").map(str::to_owned),
            model: model.to_owned(),
            pos: self.pos(node),
            fields,
            nested_in,
            csv: false,
            context_defaults: context_defaults(node),
            no_create: no_create(node),
        });
    }

    /// What a record means to the rules of its model, read from the record as a whole.
    fn model_facts(&mut self, node: Node, model: &str) {
        match model {
            "ir.actions.act_window" => {
                self.action_domain(node);
                self.action(node);
            }
            "ir.actions.act_window.view" => self.action_view(node),
            "ir.actions.server" | CRON => self.server(node, model == CRON),
            "ir.actions.report" => self.report(node),
            _ => {}
        }
        if let Some(domain) = rule_domain_field(model) {
            self.rule(node, domain);
        }
    }

    /// Odoo takes an `eval` over the text next to it, and only a run tells what that gives.
    fn action(&mut self, node: Node) {
        let last = |name: &str| last_field_of(node, name);
        let written = |name: &str| match last(name) {
            None => ActionValue::Unset,
            Some(f) if eval_of(f).is_some() => ActionValue::Opaque,
            Some(f) => literal(spec_text(f, self.lines)),
        };
        let mut action = ActionFact {
            id: node.attribute("id").map(str::to_owned),
            model: written("res_model"),
            view_mode: written("view_mode"),
            view_id: last("view_id").map_or(ActionViewId::Unset, |f| action_view_id(f, self.lines)),
            context_kinds: last("context").map(|f| context_view_kinds(eval_of(f).or_else(|| f.text()))),
            ..ActionFact::default()
        };
        let view_ids = field_of(node, "view_ids");
        if let Some(eval) = view_ids.and_then(|f| f.attribute("eval")) {
            let rows = record_eval::view_rows(eval);
            action.view_of_any_kind = rows.contains(&None);
            action.view_kinds = rows.into_iter().flatten().collect();
        }
        for row in view_ids.iter().flat_map(|f| f.children()).filter(is_view_row) {
            name_row_view(&mut action.nested_view_kinds, &mut action.nested_view_of_any_kind, row);
        }
        self.facts.actions.push(action);
    }

    /// Odoo sets the inverse field of a record nested in a one2many, whatever the record says.
    fn rule(&mut self, node: Node, domain_field: &str) {
        let (model, by_name) = match field_of(node, "model_id") {
            _ if node.parent_element().is_some_and(|p| p.has_tag_name("field")) => (ActionValue::Opaque, false),
            Some(field) => rule_model(field, self.lines),
            None => (ActionValue::Unset, false),
        };
        let domain = field_of(node, domain_field).map_or(ActionValue::Unset, |f| rule_domain(f, self.lines));
        let access = domain_field == "domain";
        let (groups, perms) = if access {
            let groups = last_field_of(node, "group_id").map(|f| access_group(f, self.lines));
            let operation = last_field_of(node, "operation");
            (
                groups,
                operation.map_or([Flag::Unset; 4], |f| operation_flags(f, self.lines)),
            )
        } else {
            let groups = last_field_of(node, "groups").map(rule_groups);
            (
                groups,
                PERM_FIELDS.map(|name| last_field_of(node, name).map_or(Flag::Unset, |f| flag(f, self.series))),
            )
        };
        self.facts.rules.push(RuleFact {
            id: node.attribute("id").map(str::to_owned),
            pos: self.pos(node),
            access,
            model,
            by_name,
            fields: domain::written_fields(&domain),
            domain,
            active: field_of(node, "active").map(|f| rule_active(f, self.series)),
            groups: groups.unwrap_or_default(),
            perms,
        });
    }

    fn server(&mut self, node: Node, cron: bool) {
        let last = |name: &str| last_field_of(node, name);
        let (model, by_name) = last("model_id").map_or((ActionValue::Unset, false), |f| rule_model(f, self.lines));
        let code = last("code");
        let (refused, calls) = code.map_or_else(Default::default, |f| self.server_code(f));
        self.facts.servers.push(ServerFact {
            id: node.attribute("id").map(str::to_owned),
            cron,
            model,
            by_name,
            parent: server_parent(node, self.lines),
            state: last("state").map(|f| !indirect_value(f) && f.text() == Some("code")),
            writes_code: code.is_some(),
            refused,
            calls,
        });
    }

    /// Reads `field` as Odoo does, which validates the stripped text up to the first child: why it
    /// refuses the code, and the calls of code it accepts.
    fn server_code(&self, field: Node) -> (Option<Spanned<String>>, Vec<Spanned<String>>) {
        let Some(text) = direct_text(field) else {
            return (None, vec![]);
        };
        let raw = text.text().unwrap_or_default();
        let lead = raw.len() - raw.trim_start().len();
        let read = server_code::read(raw.trim(), self.series);
        let spanned = |(value, at): (String, usize)| Spanned {
            value,
            pos: code_pos(text, raw, lead + at, self.lines),
        };
        (read.error.map(spanned), read.calls.into_iter().map(spanned).collect())
    }

    fn report(&mut self, node: Node) {
        let last = |name: &str| last_field_of(node, name);
        let file_name = last("print_report_name");
        let name = last("report_name");
        self.facts.reports.push(ReportFact {
            id: node.attribute("id").map(str::to_owned),
            tag: false,
            template: name.map_or(ActionValue::Unset, |f| report_target(direct_value(f, self.lines))),
            qweb: last("report_type").map(|f| !indirect_value(f) && f.text().is_some_and(qweb_report)),
            writes_file_name: file_name.is_some(),
            refused: file_name.and_then(|f| self.refused_file_name(f)),
        });
    }

    /// Print tests the value for truth, so it never compiles an empty one.
    fn refused_file_name(&self, field: Node) -> Option<Spanned<String>> {
        let text = direct_text(field)?;
        let raw = text.text().filter(|raw| !raw.is_empty())?;
        refused_expression(raw, self.series, |at| code_pos(text, raw, at, self.lines))
    }

    /// Odoo asserts `name`, `string` and `model`, so an empty `name` aborts the load and no write
    /// follows. It tests `report_type` and `print_report_name` for truth, so an empty one writes
    /// nothing.
    fn report_tag(&mut self, node: Node) {
        let set = |name: &str| attr_value(node, name, self.lines).filter(|a| !a.value.is_empty());
        let file_name = set("print_report_name");
        let refused = file_name.as_ref().and_then(|name| {
            refused_expression(&name.value, self.series, |at| Pos {
                col: name.pos.col + at as u32,
                ..name.pos
            })
        });
        self.facts.reports.push(ReportFact {
            id: node.attribute("id").map(str::to_owned),
            tag: true,
            template: set("name").map_or(ActionValue::Unset, |name| report_target(Some(name))),
            qweb: set("report_type").map(|t| qweb_report(&t.value)),
            writes_file_name: file_name.is_some(),
            refused,
        });
    }

    /// The tag Odoo reads through 16.0. It writes `view_mode`, `view_id` and `context` even without
    /// the attribute, so it clears the last two and gives the first its default.
    fn act_window(&mut self, node: Node) {
        let written = |name: &str| literal(attr_value(node, name, self.lines));
        let view_id = node.attribute("view_id").filter(|id| !id.is_empty());
        let action = ActionFact {
            id: node.attribute("id").map(str::to_owned),
            model: written("res_model"),
            view_mode: written("view_mode"),
            view_id: view_id.map_or(ActionViewId::Cleared, |id| ActionViewId::Ref(id.to_owned())),
            context_kinds: Some(context_view_kinds(node.attribute("context"))),
            ..ActionFact::default()
        };
        self.facts.actions.push(action);
    }

    /// A row of the `view_ids` of the action its `act_window_id` names without a run. Odoo overwrites
    /// that field on a row nested in `view_ids`, so such a row belongs to the enclosing action, which
    /// `action` reads.
    fn action_view(&mut self, node: Node) {
        if node.parent_element().is_some_and(|p| p.has_tag_name("field")) {
            return;
        }
        let action = last_field_of(node, "act_window_id").map(|f| written(f, self.lines));
        let Some(WrittenValue::Ids(ids)) = action else {
            return;
        };
        let [action] = ids.as_slice() else { return };
        let mut row = ActionFact {
            id: Some(action.value.clone()),
            ..ActionFact::default()
        };
        name_row_view(&mut row.view_kinds, &mut row.view_of_any_kind, node);
        if row.view_of_any_kind || !row.view_kinds.is_empty() {
            self.facts.actions.push(row);
        }
    }

    fn action_domain(&mut self, node: Node) {
        let field = |name: &str| {
            node.children()
                .find(|c| c.has_tag_name("field") && c.attribute("name") == Some(name))
        };
        let Some(model) = field("res_model").and_then(|f| f.text()).map(str::trim) else {
            return;
        };
        let mut fields = vec![];
        for (name, read) in [
            ("domain", read_domain as fn(&str) -> Vec<String>),
            ("context", domain::group_by),
        ] {
            if let Some(f) = field(name) {
                let pos = self.pos(f);
                let text = f.attribute("eval").or_else(|| f.text()).unwrap_or_default();
                fields.extend(read(text).into_iter().map(|value| Spanned { value, pos }));
            }
        }
        if !fields.is_empty() {
            self.facts.action_domains.push(DomainFact {
                model: model.to_owned(),
                fields,
            });
        }
    }

    fn record_field(&mut self, field: Node, name: &str, model: &str, view: &mut ViewFact) {
        // Odoo reads an empty ref as no value.
        if let Some(id) = field.attribute("ref").filter(|r| !r.is_empty()) {
            self.reference(id, field);
            if name == "model_id" {
                self.access_row(field, model, id);
            }
        }
        self.eval_refs(field);
        if model == "ir.ui.view" {
            self.view_field(field, name, view);
        }
        if name == "key" && KEYED_MODELS.contains(&model) {
            self.view_key(field);
        }
        if name == "view_mode" && field.text().is_some_and(|t| t.split(',').any(|m| m.trim() == "tree")) {
            let mut diag = RawDiag::new("OD306", self.pos(field), "view_mode 'tree' is renamed 'list'");
            diag.fix = listed_modes(field);
            self.facts.diags.push(diag);
        }
    }

    fn access_row(&mut self, model_id: Node, model: &str, id: &str) {
        if model == "ir.model.access" || model == "ir.access" && names_group(model_id) {
            let pos = self.pos(model_id);
            self.facts.access.push(AccessRow {
                id: None,
                model: id.to_owned(),
                by_name: false,
                pos,
                group: None,
                grants: Perms::default(),
                global_write: false,
                public_write: None,
                model_access: model == "ir.model.access",
                domain: None,
            });
        }
    }

    fn view_key(&mut self, field: Node) {
        let key = field.text().map(str::trim).filter(|k| !k.is_empty());
        self.facts.view_keys.extend(key.map(str::to_owned));
    }

    fn view_field(&mut self, field: Node, name: &str, view: &mut ViewFact) {
        match name {
            "model" => {
                let pos = self.pos(field);
                view.model = field.text().map(|t| Spanned {
                    value: t.trim().to_owned(),
                    pos,
                });
            }
            "inherit_id" => view.inherit = field.attribute("ref").filter(|r| !r.is_empty()).map(str::to_owned),
            // A `type` wins over the arch's root wherever the record sets it.
            "type" => view.kind = field.text().map(|t| t.trim().to_owned()),
            // Odoo writes `arch_base` through to `arch`.
            "arch_base" => view.arch_unread = true,
            "arch" | "arch_db" => {
                view.has_arch = true;
                for root in field.children().filter(Node::is_element) {
                    let tag = root.tag_name().name();
                    if view.kind.is_none() && !UNTYPED_ROOTS.contains(&tag) {
                        view.kind = Some(tag.to_owned());
                    }
                    self.arch(root, &mut Vec::new(), view);
                }
            }
            _ => {}
        }
    }

    fn arch(&mut self, node: Node, path: &mut Vec<String>, view: &mut ViewFact) {
        let pos = self.pos(node);
        let tag = node.tag_name().name();
        arch_element(node, pos, view);
        // Odoo reads a diagram's fields on the models its `node` and `arrow` name, not the view's.
        if tag == "field" && node.ancestors().any(|a| a.has_tag_name("diagram")) {
            return;
        }
        let depth = path.len();
        let around = self.around.len();
        let opened = spec_subview(node, path);
        if let Some(opened) = &opened {
            self.open_subview(opened, node, view);
        }
        path.extend(opened.into_iter().flatten().skip(depth));
        view.calls.extend(t_call(node, self.lines));
        self.arch_button(node, path, view);
        self.arch_modifiers(node, path, view);
        self.arch_attrs(node, pos);
        self.groups(node);
        self.arch_tag(node, tag, pos, view);
        Self::arch_markup(node, tag, pos, view);
        if tag == "field"
            && let Some(name) = node.attribute("name").filter(|n| !n.contains('.'))
        {
            self.arch_field(node, name, pos, path, view);
            return;
        }
        if tag == "filter" {
            let fields = domain_attr(node, "domain").chain(group_by_attr(node));
            arch_domain(pos, path.clone(), fields, true, view);
        }
        self.arch_context(node, tag, path, view);
        for child in node.children().filter(Node::is_element) {
            self.arch(child, path, view);
        }
        path.truncate(depth);
        self.around.truncate(around);
    }

    /// Extends `path` with where the children of `node` sit.
    fn arch_context(&mut self, node: Node, tag: &str, path: &mut Vec<String>, view: &mut ViewFact) {
        match tag {
            "xpath" => xpath_steps(node.attribute("expr").unwrap_or_default(), path, &mut self.around),
            _ if AMBIGUOUS_SPECS.contains(&tag) && node.has_attribute("position") => {
                path.push(UNKNOWN_CONTEXT.to_owned())
            }
            "groupby" => {
                if let Some(name) = node.attribute("name") {
                    path.push(name.to_owned());
                    self.open_subview(path, node, view);
                }
            }
            _ => {}
        }
    }

    fn open_subview(&mut self, path: &[String], node: Node, view: &mut ViewFact) {
        let opened = ArchSubview {
            attribute_fields: attribute_fields(node),
            ..subview(path, node.tag_name().name())
        };
        self.around.push(opened.clone());
        view.subviews.push(opened);
    }

    /// A button only calls a method of the model when it has no `special`, which Odoo reads first,
    /// and a `position` makes it a spec, whose `name` locates a button of the parent view.
    fn arch_button(&self, node: Node, path: &[String], view: &mut ViewFact) {
        if !node.has_tag_name("button")
            || node.attribute("type") != Some("object")
            || node.has_attribute("special")
            || node.has_attribute("position")
        {
            return;
        }
        if let Some(method) = attr_value(node, "name", self.lines).filter(|m| identifier(&m.value)) {
            view.buttons.push(ArchButton {
                path: path.to_vec(),
                method,
            });
        }
    }

    /// The attributes of a `position` node or an xpath locate an element, so they are no modifiers.
    fn arch_modifiers(&self, node: Node, path: &[String], view: &mut ViewFact) {
        let found: Vec<(&'static str, Spanned<String>)> = if node.has_tag_name("attribute") {
            spec_modifier(node, self.lines).into_iter().collect()
        } else if node.has_attribute("position") || node.has_tag_name("xpath") {
            vec![]
        } else {
            let read = |attr| Some((attr, attr_value(node, attr, self.lines)?));
            MODIFIERS.into_iter().filter_map(read).collect()
        };
        let joins = joined_element(node);
        let parsed = found
            .into_iter()
            .filter(|(_, value)| joins.is_some() || !modifier::constant(&value.value));
        view.modifiers.extend(parsed.map(|(attr, value)| ArchModifier {
            path: path.to_vec(),
            subviews: self.around.clone(),
            attr,
            value,
            joins: joins.clone(),
        }));
    }

    fn arch_attrs(&mut self, node: Node, pos: Pos) {
        for attr in node.attributes() {
            match attr.name() {
                "attrs" | "states" => self.facts.diags.push(RawDiag::new(
                    "OD305",
                    pos,
                    format!(
                        "the {} attribute is removed; write invisible, readonly or required as a Python expression",
                        attr.name()
                    ),
                )),
                "t-raw" => self.facts.diags.push(RawDiag::new(
                    "OD307",
                    pos,
                    "t-raw renders unescaped HTML; use t-out and wrap trusted HTML in Markup",
                )),
                _ => {
                    for cap in ACTION_REF.captures_iter(attr.value()) {
                        self.reference(&cap[1], node);
                    }
                }
            }
        }
    }

    fn arch_tag(&mut self, node: Node, tag: &str, pos: Pos, view: &mut ViewFact) {
        match tag {
            "tree" => {
                let mut diag = RawDiag::new("OD306", pos, "the <tree> view is renamed <list>");
                diag.fix = rename_tag(node, "list");
                view.renamed.push(diag);
            }
            "attribute" if matches!(node.attribute("name"), Some("attrs" | "states")) => {
                self.facts.diags.push(RawDiag::new("OD305", pos, "the attrs and states attributes are removed; write invisible, readonly or required as a Python expression"));
            }
            _ => {}
        }
    }

    /// OD326 and OD327 on markup the web client stops reading, and the `t-esc` that OD325 checks once
    /// it knows which tree the view belongs to. A spec locates the parent's markup rather than adding
    /// its own.
    fn arch_markup(node: Node, tag: &str, pos: Pos, view: &mut ViewFact) {
        if locates(node) {
            return;
        }
        if node.has_attribute("t-esc") {
            view.escapes.push(escape(node, pos));
        }
        if node.attribute("t-name") == Some("kanban-box") && !beside_card(node) {
            view.dropped
                .push(RawDiag::new("OD326", pos, "the kanban-box template is renamed card"));
        }
        if tag == "div" && has_class(node, "oe_chatter") {
            view.dropped.push(RawDiag::new(
                "OD327",
                pos,
                "a div with class oe_chatter renders as a plain div, without the chatter; write <chatter/>",
            ));
        }
    }

    fn arch_field(&mut self, node: Node, name: &str, pos: Pos, path: &mut Vec<String>, view: &mut ViewFact) {
        if node.has_attribute("position") {
            // A locator matches the first such field anywhere in the parent arch, subviews included.
            path.push(format!("{LOCATOR}{name}"));
            for child in node.children().filter(Node::is_element) {
                self.arch(child, path, view);
            }
            path.pop();
            return;
        }
        let field_path: Vec<String> = path.iter().cloned().chain([name.to_owned()]).collect();
        // A field's domain filters its comodel; its filter_domain, in a search view, the view's model.
        arch_domain(pos, field_path.clone(), domain_attr(node, "domain"), true, view);
        arch_domain(pos, path.clone(), domain_attr(node, "filter_domain"), false, view);
        view.fields.push(ArchField { path: field_path, pos });
        for child in node.children().filter(Node::is_element) {
            let tag = child.tag_name().name();
            if SUBVIEWS.contains(&tag) {
                path.push(name.to_owned());
                self.open_subview(path, child, view);
                self.arch(child, path, view);
                self.around.pop();
                path.pop();
            } else {
                self.arch(child, path, view);
            }
        }
    }
}

/// Whether the kanban a template is in, or failing one the arch, also defines `card`, which the
/// client then draws instead, so `kanban-box` is never read. A spec on `card` finds it in the parent.
fn beside_card(template: Node) -> bool {
    let arch = |a: &Node| a.has_tag_name("field") && matches!(a.attribute("name"), Some("arch" | "arch_db"));
    let scope = template
        .ancestors()
        .find(|a| a.has_tag_name("kanban"))
        .or_else(|| template.ancestors().find(arch));
    scope.is_some_and(|s| s.descendants().any(|d| d.attribute("t-name") == Some("card")))
}

fn has_class(node: Node, class: &str) -> bool {
    node.attribute("class")
        .is_some_and(|c| c.split_whitespace().any(|token| token == class))
}

fn field_of<'a, 'i>(node: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    node.children()
        .find(|c| c.has_tag_name("field") && c.attribute("name") == Some(name))
}

/// What `field` writes, in the order Odoo reads it: a `search`, a `ref`, an `eval`, then the text.
/// An empty attribute counts as absent. An empty text gives no value to check: it clears a
/// selection below saas~19.3, and from there writes `''`, which OD322 leaves unreported.
fn written(field: Node, lines: &Lines) -> WrittenValue {
    let set = |attr: &str| attr_value(field, attr, lines).filter(|a| !a.value.is_empty());
    if set("search").is_some() {
        return searched_ir_model(field, lines).map_or(WrittenValue::Opaque, WrittenValue::Model);
    }
    if let Some(id) = set("ref") {
        return WrittenValue::Ids(vec![id]);
    }
    match set("eval") {
        Some(eval) => record_eval::written(&eval),
        None => direct_value(field, lines).map_or(WrittenValue::Opaque, WrittenValue::Text),
    }
}

/// The `eval` of `field` that Odoo runs: it tests the attribute for truth, so an empty one leaves
/// the text.
fn eval_of<'a>(field: Node<'a, '_>) -> Option<&'a str> {
    field.attribute("eval").filter(|eval| !eval.is_empty())
}

/// Whether `field` adds to what the record holds: a `search` or a `ref` wins over an `eval`, and Odoo
/// writes nothing to a field for the records nested in it.
fn incremental(field: Node) -> bool {
    let by_commands = !["search", "ref"]
        .iter()
        .any(|a| field.attribute(*a).is_some_and(|v| !v.is_empty()))
        && field.attribute("eval").is_some_and(record_eval::incremental);
    by_commands || field.children().any(|c| c.has_tag_name("record"))
}

/// The fields `default_*` keys name where Odoo reads a `context` for `node`: on it, and on the roots
/// around it. An enclosing `<record>` passes its own to nothing nested in it. Any mention counts, as
/// what a context evaluates to takes a run.
fn context_defaults(node: Node) -> Vec<String> {
    let roots = node
        .ancestors()
        .skip(1)
        .filter(|n| !n.has_tag_name("record") && !n.has_tag_name("field"));
    let contexts = std::iter::once(node)
        .chain(roots)
        .filter_map(|n| n.attribute("context"));
    let keys = contexts.flat_map(|c| CONTEXT_DEFAULT.captures_iter(c));
    keys.map(|c| c[1].to_owned()).collect()
}

/// Whether `forcecreate` reads as false, as Odoo reads it: an empty value keeps the default.
fn no_create(node: Node) -> bool {
    let value = node.attribute("forcecreate").map(|v| v.trim().to_lowercase());
    value.is_some_and(|v| matches!(v.as_str(), "0" | "false" | "off"))
}

/// The model and the field of an `ir.default.set()`, whose arguments are those of the `eval` then
/// the unnamed `<value>` children.
fn default_of(node: Node) -> Option<(String, String)> {
    if node.attribute("model") != Some("ir.default") || node.attribute("name") != Some("set") {
        return None;
    }
    let mut args = node.attribute("eval").map(record_eval::strings).unwrap_or_default();
    let values = node
        .children()
        .filter(|c| c.has_tag_name("value") && !c.has_attribute("name"));
    args.extend(values.map(|v| v.text().map(str::to_owned)));
    Some((args.first()?.clone()?, args.get(1)?.clone()?))
}

/// The field Odoo keeps of those of a name: each one replaces the value of the one before.
fn last_field_of<'a, 'i>(node: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    node.children()
        .rfind(|c| c.has_tag_name("field") && c.attribute("name") == Some(name))
}

/// Whether Odoo stores a `<record>` of `ir.ui.view` as primary, or None when only a run can tell. A
/// `mode` wins, as `inherit_id` only sets its default, and a `search` that finds nothing sets none.
fn primary_view(node: Node, lines: &Lines) -> Option<bool> {
    if let Some(mode) = last_field_of(node, "mode") {
        return match written(mode, lines) {
            WrittenValue::Text(text) => Some(text.value != "extension"),
            _ => None,
        };
    }
    let Some(parent) = last_field_of(node, "inherit_id").filter(|f| holds_record(*f)) else {
        return Some(true);
    };
    match written(parent, lines) {
        WrittenValue::Ids(_) | WrittenValue::Text(_) => Some(false),
        _ => None,
    }
}

/// The field holding the domain of a model of record rules.
pub fn rule_domain_field(model: &str) -> Option<&'static str> {
    match model {
        "ir.rule" => Some("domain_force"),
        "ir.access" => Some("domain"),
        _ => None,
    }
}

/// Whether Odoo takes the value of `field` from elsewhere than its text. It tests each attribute for
/// truth, an empty one included, but reads the text by `type` whenever that attribute is there.
fn indirect_value(field: Node) -> bool {
    let set = |attr: &&str| field.attribute(*attr).is_some_and(|v| *attr == "type" || !v.is_empty());
    INDIRECT_VALUE.iter().chain(&["eval"]).any(set)
}

/// The text Odoo reads from `field`, up to its first child, unless it takes the value from elsewhere.
fn direct_text<'a, 'i>(field: Node<'a, 'i>) -> Option<Node<'a, 'i>> {
    field.first_child().filter(|t| t.is_text() && !indirect_value(field))
}

/// What `direct_text` holds, at its first character.
fn direct_value(field: Node, lines: &Lines) -> Option<Spanned<String>> {
    let text = direct_text(field)?;
    let raw = text.text()?;
    Some(Spanned {
        value: raw.to_owned(),
        pos: code_pos(text, raw, raw.len() - raw.trim_start().len(), lines),
    })
}

/// A `report_name` no scan can look up holds no literal, as the target of a `t-call`.
fn report_target(name: Option<Spanned<String>>) -> ActionValue {
    let trimmed = name.map(|n| Spanned {
        value: n.value.trim().to_owned(),
        ..n
    });
    literal(trimmed.filter(|n| literal_target(&n.value)))
}

fn qweb_report(report_type: &str) -> bool {
    report_type.starts_with(QWEB_REPORT)
}

/// Why Odoo refuses `text` as an expression, which it strips first, where `at` places a byte of it.
fn refused_expression(text: &str, series: Option<Series>, at: impl Fn(usize) -> Pos) -> Option<Spanned<String>> {
    let lead = text.len() - text.trim_start().len();
    let (value, offset) = server_code::expression_error(text.trim(), series)?;
    Some(Spanned {
        value,
        pos: at(lead + offset),
    })
}

/// Whether the `ir.access` record of a `model_id` field gives `group_id` a value. Without one the
/// record restricts every user rather than granting.
fn names_group(model_id: Node) -> bool {
    let group = model_id
        .parent_element()
        .and_then(|record| field_of(record, "group_id"));
    group.is_some_and(|f| {
        let blank = |attr: &str| f.attribute(attr).is_none_or(str::is_empty);
        !blank("ref") || !blank("eval") || !blank("search") || f.text().is_some_and(|t| !t.trim().is_empty())
    })
}

/// The model a `search` of `field` finds, when it searches `ir.model` by name and writes the id of
/// the record it finds.
fn searched_ir_model(field: Node, lines: &Lines) -> Option<Spanned<String>> {
    if field.has_attribute("use") || field.attribute("model").is_some_and(|m| m != "ir.model") {
        return None;
    }
    let search = attr_value(field, "search", lines)?;
    let value = domain::searched_model(&search.value)?;
    Some(Spanned { value, pos: search.pos })
}

/// What a `model_id` field writes, as Odoo reads it: a `search`, a `ref`, then an `eval`. With
/// `true` the value is a model name, else an XML id.
fn rule_model(field: Node, lines: &Lines) -> (ActionValue, bool) {
    let opaque = (ActionValue::Opaque, false);
    if field.has_attribute("use") || field.attribute("model").is_some_and(|m| m != "ir.model") {
        return opaque;
    }
    if attr_value(field, "search", lines).is_some_and(|s| !s.value.is_empty()) {
        return searched_ir_model(field, lines).map_or(opaque, |model| (ActionValue::Literal(model), true));
    }
    if let Some(id) = attr_value(field, "ref", lines).filter(|r| !r.value.is_empty()) {
        return (ActionValue::Literal(id), false);
    }
    let Some(eval) = attr_value(field, "eval", lines) else {
        return opaque;
    };
    let id = WHOLE_EVAL_REF.captures(&eval.value).map(|c| c[1].to_owned());
    id.map_or(opaque, |value| {
        let pos = eval.pos;
        (ActionValue::Literal(Spanned { value, pos }), false)
    })
}

/// Only an `eval` or the text tells a domain without a run. What `ir.rule` reads as no domain is
/// a literal empty one.
fn rule_domain(field: Node, lines: &Lines) -> ActionValue {
    if INDIRECT_VALUE.iter().any(|a| field.has_attribute(*a)) {
        return ActionValue::Opaque;
    }
    if writes_no_domain(field) {
        let pos = lines.pos(field.range().start);
        return ActionValue::Literal(Spanned {
            value: String::new(),
            pos,
        });
    }
    literal(attr_value(field, "eval", lines).or_else(|| spec_text(field, lines)))
}

/// Whether `field` writes a false or empty domain: an empty text (an empty `eval` reads the text),
/// or an `eval` of `False`, `None` or `''`. A blank text or the text `False` is stored as written
/// and fails the domain check (18.0 `odoo/addons/base/models/ir_rule.py:61-69`).
fn writes_no_domain(field: Node) -> bool {
    match field.attribute("eval").filter(|e| !e.is_empty()) {
        Some(eval) => matches!(eval.trim(), "False" | "None" | "''" | "\"\""),
        None => !field.has_children(),
    }
}

/// The fields of an `ir.rule` that say which operations it applies to, in the order of `Perms`.
pub const PERM_FIELDS: [&str; 4] = ["perm_read", "perm_write", "perm_create", "perm_unlink"];

/// The first series that converts the string a boolean field gets from a data file
/// (`odoo/tools/convert.py:197-198,704-705` on 10.0). 9.0 stores the string's truth.
const BOOL_TEXT_CONVERTED: Series = Series { major: 10, minor: 0 };

/// The first series that reads an empty field as the empty string (`pycompat.to_text`,
/// `odoo/tools/convert.py:152` on 11.0). Before, it gives `None`, which no conversion takes.
const EMPTY_TEXT_AS_STRING: Series = Series { major: 11, minor: 0 };

/// The operations an `ir.access` spells as letters of `operation`, in the order of `Perms`.
pub fn operation_perms(operation: &str) -> [Flag; 4] {
    ['r', 'u', 'c', 'd'].map(|letter| Flag::Set(operation.contains(letter)))
}

fn operation_flags(field: Node, lines: &Lines) -> [Flag; 4] {
    match written(field, lines) {
        WrittenValue::Text(text) => operation_perms(&text.value),
        _ => [Flag::Opaque; 4],
    }
}

/// What a `<field>` gives `group_id`, a many2one: one id, none for a false `eval`.
fn access_group(field: Node, lines: &Lines) -> RuleGroups {
    let replaced = |added| RuleGroups {
        added,
        keeps: false,
        opaque: false,
    };
    match written(field, lines) {
        WrittenValue::Ids(mut ids) if ids.len() == 1 => replaced(vec![ids.remove(0).value]),
        _ if eval_of(field).is_some_and(record_eval::is_false) => replaced(vec![]),
        _ => RuleGroups {
            added: vec![],
            keeps: false,
            opaque: true,
        },
    }
}

/// What a `<field>` gives `groups`, a many2many: only the commands of an `eval` tell it.
fn rule_groups(field: Node) -> RuleGroups {
    let linked = eval_of(field).filter(|_| !INDIRECT_VALUE.iter().any(|a| field.has_attribute(*a)));
    match linked.and_then(record_eval::linked) {
        Some(linked) => RuleGroups {
            added: linked.ids,
            keeps: linked.keeps,
            opaque: false,
        },
        None => RuleGroups {
            added: vec![],
            keeps: false,
            opaque: true,
        },
    }
}

/// The parent a `<record>` gives a server action. Odoo sets the inverse of a one2many over whatever
/// the record nested in it says, unless the enclosing id names no `ir.actions.server` record.
fn server_parent(node: Node, lines: &Lines) -> ActionValue {
    let enclosing = node.parent_element().filter(|p| p.has_tag_name("field"));
    if let Some(field) = enclosing.filter(|f| f.attribute("name") == Some("child_ids")) {
        let parent = field.parent_element();
        let parent = parent.filter(|r| r.attribute("model") == Some("ir.actions.server"));
        return literal(parent.and_then(|r| attr_value(r, "id", lines)));
    }
    let Some(field) = last_field_of(node, "parent_id") else {
        return ActionValue::Unset;
    };
    match written(field, lines) {
        WrittenValue::Ids(mut ids) if ids.len() == 1 => ActionValue::Literal(ids.remove(0)),
        _ => ActionValue::Opaque,
    }
}

/// An `eval` that is no constant counts as false.
fn rule_active(field: Node, series: Option<Series>) -> bool {
    flag(field, series) == Flag::Set(true)
}

/// What `field` gives a boolean. A string, from the text or a string literal `eval`, reads as
/// `stored_bool` says; a string `eval` is tested before `is_false`, which takes `''` for false.
fn flag(field: Node, series: Option<Series>) -> Flag {
    if INDIRECT_VALUE.iter().any(|a| field.has_attribute(*a)) {
        return Flag::Opaque;
    }
    match field.attribute("eval") {
        Some(eval) if matches!(eval.trim(), "True" | "1") => Flag::Set(true),
        Some(eval) => match record_eval::literal_string(eval) {
            Some(text) => Flag::Set(stored_bool(&text, series)),
            None if record_eval::is_false(eval) => Flag::Set(false),
            None => Flag::Opaque,
        },
        None => match field.text().filter(|text| !text.is_empty()) {
            Some(text) => Flag::Set(stored_bool(text, series)),
            None => Flag::Set(series.is_none_or(|series| series >= EMPTY_TEXT_AS_STRING)),
        },
    }
}

/// The boolean Odoo stores for a string. From `BOOL_TEXT_CONVERTED` it tests the string untrimmed
/// against three spellings in any case, so an empty one is true; before, the field stores the
/// string's truth (`openerp/osv/fields.py:241` on 9.0). An unknown series reads as the newer one.
/// An empty field is no string before `EMPTY_TEXT_AS_STRING`, and `flag` reads it apart.
fn stored_bool(text: &str, series: Option<Series>) -> bool {
    if series.is_some_and(|series| series < BOOL_TEXT_CONVERTED) {
        return !text.is_empty();
    }
    !matches!(text.to_lowercase().as_str(), "0" | "false" | "off")
}

/// The value of a field of an action that is present: a blank one holds no literal.
pub fn literal(text: Option<Spanned<String>>) -> ActionValue {
    text.filter(|t| !t.value.trim().is_empty())
        .map_or(ActionValue::Opaque, ActionValue::Literal)
}

fn is_view_row(node: &Node) -> bool {
    node.has_tag_name("record") && node.attribute("model") == Some("ir.actions.act_window.view")
}

/// Adds what a row of `view_ids` names. A row without a view names none, so Odoo still
/// looks a view up for it.
fn name_row_view(kinds: &mut Vec<String>, of_any_kind: &mut bool, row: Node) {
    if !last_field_of(row, "view_id").is_some_and(holds_record) {
        return;
    }
    let kind = last_field_of(row, "view_mode").and_then(|f| f.text()).map(str::trim);
    match kind.filter(|k| !k.is_empty()) {
        Some(kind) => kinds.push(kind.to_owned()),
        None => *of_any_kind = true,
    }
}

fn action_view_id(field: Node, lines: &Lines) -> ActionViewId {
    if !holds_record(field) {
        return ActionViewId::Cleared;
    }
    match written(field, lines) {
        WrittenValue::Ids(mut ids) if ids.len() == 1 => ActionViewId::Ref(ids.remove(0).value),
        _ => ActionViewId::Unread,
    }
}

fn context_view_kinds(context: Option<&str>) -> Vec<String> {
    let keys = CONTEXT_VIEW_REF.captures_iter(context.unwrap_or_default());
    keys.map(|c| c[1].to_owned()).collect()
}

fn attr_value(node: Node, name: &str, lines: &Lines) -> Option<Spanned<String>> {
    let attr = node.attribute_node(name)?;
    Some(Spanned {
        value: attr.value().to_owned(),
        pos: lines.pos(attr.range_value().start),
    })
}

fn filled(node: Node, attr: &str) -> bool {
    node.attribute(attr).is_some_and(|v| !v.is_empty())
}

/// The modifier the `<attribute>` spec `node` may leave empty: a `remove` may take its whole value,
/// and without `add` or `remove` the text replaces it.
fn emptied_modifier(node: Node) -> Option<ArchEmptied> {
    let cleared = !filled(node, "add") && node.text().unwrap_or_default().is_empty();
    let attr = MODIFIERS.into_iter().find(|a| node.attribute("name") == Some(a))?;
    (filled(node, "remove") || cleared).then(|| ArchEmptied {
        attr,
        target: spec_target(node),
    })
}

/// The element whose value an `add` of the `<attribute>` spec `node` joins. Only that value tells
/// whether the `add` stands alone, and a `remove` may empty it first.
fn joined_element(node: Node) -> Option<ArchElement> {
    if !node.has_tag_name("attribute") || !filled(node, "add") || filled(node, "remove") {
        return None;
    }
    spec_target(node)
}

/// The element the spec around the `<attribute>` `node` names.
fn spec_target(node: Node) -> Option<ArchElement> {
    let spec = node.parent_element()?;
    match spec.tag_name().name() {
        "xpath" => xpath_target(spec.attribute("expr")?),
        _ => named_elements(spec).next(),
    }
}

/// The expression an `<attribute>` spec gives a modifier: its text, or an `add`, which Odoo joins to
/// the value it finds by `and` or `or`, so it reads alone. Any other separator splits the value at
/// places only that value tells.
fn spec_modifier(node: Node, lines: &Lines) -> Option<(&'static str, Spanned<String>)> {
    let attr = MODIFIERS.into_iter().find(|a| node.attribute("name") == Some(a))?;
    let joined = matches!(node.attribute("separator").map(str::trim), Some("and" | "or"));
    let value = match node.has_attribute("add") {
        true if joined => attr_value(node, "add", lines)?,
        true => return None,
        false => spec_raw(node, lines)?,
    };
    Some((attr, value))
}

fn identifier(name: &str) -> bool {
    let start = name.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_');
    start && name.chars().all(|c| c.is_alphanumeric() || c == '_')
}

fn read_domain(text: &str) -> Vec<String> {
    domain::parse(text).unwrap_or_default()
}

fn domain_attr(node: Node, attr: &str) -> impl Iterator<Item = String> {
    node.attribute(attr).map(read_domain).unwrap_or_default().into_iter()
}

fn group_by_attr(node: Node) -> impl Iterator<Item = String> {
    node.attribute("context")
        .map(domain::group_by)
        .unwrap_or_default()
        .into_iter()
}

fn arch_domain(
    pos: Pos,
    path: Vec<String>,
    fields: impl Iterator<Item = String>,
    validated: bool,
    view: &mut ViewFact,
) {
    let fields: Vec<Spanned<String>> = fields.map(|value| Spanned { value, pos }).collect();
    if !fields.is_empty() {
        view.domains.push(ArchDomain {
            path,
            fields,
            validated,
        });
    }
}

/// Records `node` as a locator when it is a spec, or else as an element views extending this one
/// can locate. A spec on another tag than field matches every attribute it carries, so checking one
/// that names the element is a necessary condition.
fn arch_element(node: Node, pos: Pos, view: &mut ViewFact) {
    old_names(node, view);
    match node.tag_name().name() {
        "xpath" => view
            .locators
            .extend(xpath_locators(node.attribute("expr").unwrap_or_default(), pos)),
        // An attribute spec can rename the element it locates; any other attribute it sets is no element.
        "attribute" => {
            view.elements.extend(renamed(node));
            view.emptied.extend(emptied_modifier(node));
        }
        _ if locates(node) => view
            .locators
            .extend(named_elements(node).take(1).map(|value| Spanned { value, pos })),
        _ => added_element(node, view),
    }
}

/// `node` as each attribute that names it locates it, in the order of `LOCATING_ATTRS`.
fn named_elements(node: Node) -> impl Iterator<Item = ArchElement> {
    LOCATING_ATTRS.into_iter().filter_map(move |attr| {
        Some(ArchElement {
            tag: node.tag_name().name().to_owned(),
            attr: attr.to_owned(),
            value: node.attribute(attr)?.to_owned(),
        })
    })
}

fn added_element(node: Node, view: &mut ViewFact) {
    let elements: Vec<ArchElement> = named_elements(node).collect();
    if elements.is_empty() {
        return;
    }
    view.elements.extend(elements.iter().cloned());
    view.valued.push(ArchValued {
        elements,
        attrs: MODIFIERS.into_iter().filter(|attr| filled(node, attr)).collect(),
    });
}

/// The element an `<attribute>` spec gives a new value of an attribute that locates it.
fn renamed(node: Node) -> Option<ArchElement> {
    let attr = LOCATING_ATTRS.into_iter().find(|a| node.attribute("name") == Some(a))?;
    Some(ArchElement {
        tag: spec_tag(node.parent_element()?)?.to_owned(),
        attr: attr.to_owned(),
        value: node.text()?.trim().to_owned(),
    })
}

/// Notes a spec that finds the parent's element by a name OD306 or OD325 rewrites, which keeps the
/// tree from being fixed, and a `<tree>` the arch adds, which a `<tree>` spec may find. An
/// `<attribute>` setting `t-esc` or `t-field` puts it beside the parent's renamed `t-out`, which
/// 20.0 `_compile_directive_out` reads first (`ir_qweb.py:2476-2480`), so the child's value would
/// be lost. A `t-raw` renders nothing from saas~19.1 with or without the rename.
fn old_names(node: Node, view: &mut ViewFact) {
    if node.has_tag_name("xpath") {
        let expr = node.attribute("expr").unwrap_or_default();
        view.tree_paths |=
            XPATH_TREE.is_match(expr) || (XPATH_NAME_CALL.is_match(expr) && XPATH_TREE_STRING.is_match(expr));
        view.escape_specs |= expr.contains("t-esc");
    } else if node.has_tag_name("attribute") {
        view.escape_specs |= meets_escape(node);
    } else if locates(node) {
        view.tree_specs |= node.has_tag_name("tree");
        view.escape_specs |= node.has_attribute("t-esc");
    } else {
        view.tree_elements |= node.has_tag_name("tree");
    }
}

/// Whether the `<attribute>` spec `node` meets a parent's `t-esc` once renamed. A `t-out` spec
/// setting plain text replaces the parent's value either way, but a non-empty `add` or `remove`
/// joins it (`x,y`, not `y`) and an empty one deletes it (20.0 `template_inheritance.py:256-311`).
fn meets_escape(node: Node) -> bool {
    let name = node.attribute("name").unwrap_or_default();
    let text = !filled(node, "add") && !filled(node, "remove") && !node.text().unwrap_or_default().is_empty();
    OUT_BESIDE_ESCAPE.contains(&name) || (name == "t-out" && !text)
}

/// OD325 on `node`. 19.0 `_compile_directive_out` renders the first of `t-out`, `t-field`, `t-esc`
/// and `t-raw` the element carries (`ir_qweb.py:2363-2372`), so beside another one the rename would
/// repeat `t-out` or change which one renders.
fn escape(node: Node, pos: Pos) -> RawDiag {
    let mut diag = RawDiag::new(
        "OD325",
        pos,
        "the server no longer knows t-esc and renders nothing for it; write t-out",
    );
    let alone = !["t-out", "t-field", "t-raw"].iter().any(|a| node.has_attribute(*a));
    diag.fix = alone.then(|| rename_attr(node, "t-esc", "t-out")).flatten();
    diag
}

/// Whether the text of `node` is in the document's body: what a DOCTYPE entity expands to has the
/// offsets of the entity's definition.
fn in_body(node: Node) -> bool {
    node.range().start >= node.document().root_element().range().start
}

/// The edit writing `to` over `from` at `start` of `text`, when `from` is there.
fn renaming(text: &str, start: usize, from: &str, to: &str) -> Option<Edit> {
    (text.get(start..start + from.len())? == from).then(|| Edit {
        start,
        end: start + from.len(),
        expected: from.to_owned(),
        replacement: to.to_owned(),
    })
}

/// The fix renaming element `node` to `to`, in its opening tag and its closing one if any. A
/// prefixed name stays, since the text holds the prefix where the bare name is looked for.
fn rename_tag(node: Node, to: &str) -> Option<Fix> {
    let text = node.document().input_text();
    let (range, name) = (node.range(), node.tag_name().name());
    let open = range.start + 1;
    let ends_name = |b: &u8| matches!(b, b' ' | b'\t' | b'\r' | b'\n' | b'/' | b'>');
    if !in_body(node) || !text.as_bytes().get(open + name.len()).is_some_and(ends_name) {
        return None;
    }
    let mut edits = vec![renaming(text, open, name, to)?];
    let element = text.get(range.clone())?;
    if !element.ends_with("/>") {
        let close = element.strip_suffix('>')?.trim_end_matches([' ', '\t', '\r', '\n']);
        let start = range.start + close.strip_suffix(name)?.strip_suffix("</")?.len() + 2;
        edits.push(renaming(text, start, name, to)?);
    }
    Some(Fix { edits })
}

/// The fix renaming attribute `from` of `node` to `to`.
fn rename_attr(node: Node, from: &str, to: &str) -> Option<Fix> {
    let attr = node
        .attributes()
        .find(|a| a.name() == from && a.namespace().is_none())?;
    let start = attr.range_qname().start;
    in_body(node)
        .then(|| renaming(node.document().input_text(), start, from, to))
        .flatten()
        .map(Fix::single)
}

/// The fix renaming each `tree` of a `view_mode` to `list`. Odoo refuses a mode listed twice (18.0
/// `ir_actions.py:293-298`, 20.0 `:283-287`), so a `view_mode` that lists `list` stays.
fn listed_modes(field: Node) -> Option<Fix> {
    if INDIRECT_VALUE.iter().chain(&["eval"]).any(|a| field.has_attribute(*a)) {
        return None;
    }
    let (input, range) = plain_text(field)?;
    let modes: Vec<&str> = input[range.clone()].split(',').collect();
    if modes.iter().any(|m| m.trim() == "list") {
        return None;
    }
    let mut at = range.start;
    let mut edits = vec![];
    for mode in modes {
        let lead = mode.len() - mode.trim_start().len();
        if mode.trim() == "tree" {
            edits.extend(renaming(input, at + lead, "tree", "list"));
        }
        at += mode.len() + 1;
    }
    (!edits.is_empty()).then_some(Fix { edits })
}

/// The document's text and the range of the text of `field` in it, when that text is the field's
/// only content, closed by its end tag, and spelled as Odoo reads it: no entity and no CDATA.
fn plain_text<'i>(field: Node<'_, 'i>) -> Option<(&'i str, Range<usize>)> {
    let mut children = field.children();
    let node = children.next().filter(Node::is_text)?;
    let input = node.document().input_text();
    let range = node.range();
    let raw = input.get(range.clone())?;
    let spelled = node.text() == Some(raw) && !raw.contains('&');
    let closed = input[range.end..].starts_with("</");
    (spelled && closed && children.next().is_none() && in_body(node)).then_some((input, range))
}

/// Whether Odoo looks `node` up in the parent view rather than adding it: a spec, or the child of a
/// spec that `position="move"` takes from elsewhere in that view.
fn locates(node: Node) -> bool {
    spec(node) || node.attribute("position") == Some("move") && node.parent_element().is_some_and(spec)
}

/// Whether `node` is a spec of an inheriting view, which only its place tells: a spec needs no
/// `position`, and `position` elsewhere is a parameter of a `t-call` from 19.0 on. `<data>` lends
/// its children to the inheritance and locates nothing itself.
fn spec(node: Node) -> bool {
    let owner = node.ancestors().skip(1).find(|a| !a.has_tag_name("data"));
    !node.has_tag_name("data") && owner.is_some_and(inheriting_root)
}

/// Whether the children of `node` are specs. A record may write `inherit_id` after its arch, and
/// writing it does not give the record a parent.
fn inheriting_root(node: Node) -> bool {
    let record_arch = || {
        node.has_tag_name("field")
            && matches!(node.attribute("name"), Some("arch" | "arch_db"))
            && node
                .parent_element()
                .is_some_and(|r| r.has_tag_name("record") && field_of(r, "inherit_id").is_some_and(holds_record))
    };
    node.has_attribute("t-inherit") || node.attribute("inherit_id").is_some_and(|p| !p.is_empty()) || record_arch()
}

/// Whether a many2one field holds a record, read in the order of `written`. A `search` that may find
/// nothing and an `eval` only a run can tell count, as they are written to give one.
fn holds_record(field: Node) -> bool {
    let set = |attr: &str| field.attribute(attr).filter(|v| !v.is_empty());
    if set("search").is_some() || set("ref").is_some() {
        return true;
    }
    match set("eval") {
        Some(eval) => !record_eval::is_false(eval),
        None => field.text().is_some_and(|t| !t.trim().is_empty()),
    }
}

/// The template a node calls by a literal name: its `t-call` at the value, unless that locates a
/// call of the parent view, or the text of an `<attribute name="t-call">` spec. An empty, numeric,
/// formatted or path target names no template a scan can look up: Odoo reads digits as a view id,
/// and `{{ }}` and `#{}` only at render time.
pub fn t_call(node: Node, lines: &Lines) -> Option<Spanned<String>> {
    let target = if node.has_tag_name("attribute") && node.attribute("name") == Some("t-call") {
        spec_text(node, lines)?
    } else if node.has_attribute("t-call") && !locates(node) {
        attr_value(node, "t-call", lines)?
    } else {
        return None;
    };
    literal_target(&target.value).then_some(target)
}

/// The path of a script Odoo aggregates into the bundle a template renders, by its non-empty
/// segments: a URL with no scheme nor host, outside `/web/content` (14.0
/// `odoo/addons/base/models/ir_qweb.py:313-314`, `:349`). As werkzeug's `url_parse` reads them, a
/// scheme is letters, digits or `+,-.` before a `:` no port number follows, and a host a non-empty
/// name after `//`.
fn aggregated(src: &str) -> Option<String> {
    let scheme = src.split_once(':').is_some_and(|(scheme, rest)| {
        let named = !scheme.is_empty()
            && scheme
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"+,-.".contains(&b));
        named && (rest.is_empty() || !rest.bytes().all(|b| b.is_ascii_digit()))
    });
    let host = src
        .strip_prefix("//")
        .is_some_and(|r| !r.is_empty() && !r.starts_with(['/', '?', '#']));
    if scheme || host || src.starts_with("/web/content") {
        return None;
    }
    let path: Vec<&str> = src.split('/').filter(|s| !s.is_empty()).collect();
    (!path.is_empty()).then(|| path.join("/"))
}

/// The elements of `node` that render at the top of the output, which alone Odoo reads a bundle's
/// scripts from: a `<t>` renders only its content, while a `<t t-call>` renders another template
/// and any other element keeps what it holds (14.0 `odoo/addons/base/models/ir_qweb.py:328-356`).
fn rendered<'a, 'i>(node: Node<'a, 'i>) -> Vec<Node<'a, 'i>> {
    let mut top = vec![];
    for child in node.children().filter(Node::is_element) {
        if child.has_tag_name("t") && child.attribute("t-call").is_none() {
            top.extend(rendered(child));
        } else {
            top.push(child);
        }
    }
    top
}

/// What an element a template adds puts in its bundle: an aggregated script, or through a literal
/// `t-call` what the called template renders.
fn asset_of(node: Node) -> Option<AssetEntry> {
    if !node.is_element() || locates(node) {
        return None;
    }
    let (directive, path) = if node.has_tag_name("script") {
        (Directive::Append, aggregated(node.attribute("src")?)?)
    } else {
        let call = node.attribute("t-call").filter(|c| literal_target(c))?;
        (Directive::Include, call.to_owned())
    };
    Some(AssetEntry {
        directive,
        target: None,
        path,
    })
}

/// What the specs of an extending template, through `<data>`, put in its bundle. A spec placed
/// against a script it locates chains its scripts from there, and replacing with no script removes
/// it; any other spec appends, as the rendered order is out of reach.
fn extension_assets(node: Node, assets: &mut Vec<AssetEntry>) {
    for spec in node.children().filter(Node::is_element) {
        if spec.has_tag_name("data") {
            extension_assets(spec, assets);
            continue;
        }
        let added = rendered(spec).into_iter().filter_map(asset_of);
        let mut directive = match spec.attribute("position").unwrap_or("inside") {
            "before" => Directive::Before,
            "after" => Directive::After,
            "replace" => Directive::Replace,
            _ => Directive::Append,
        };
        let Some(target) = located_script(spec).filter(|_| directive != Directive::Append) else {
            assets.extend(added);
            continue;
        };
        let mut anchor = target.clone();
        for mut entry in added {
            if entry.directive == Directive::Append {
                entry.directive = directive;
                entry.target = Some(std::mem::replace(&mut anchor, entry.path.clone()));
                directive = Directive::After;
            }
            assets.push(entry);
        }
        if directive == Directive::Replace {
            assets.push(AssetEntry {
                directive: Directive::Remove,
                target: None,
                path: target,
            });
        }
    }
}

/// The aggregated script a spec locates: a `<script>` spec by its `src`, or an xpath whose last step
/// names one by its `src`.
fn located_script(spec: Node) -> Option<String> {
    let src = match spec.tag_name().name() {
        "script" => spec.attribute("src")?,
        "xpath" => {
            let captures = XPATH_SCRIPT.captures(spec.attribute("expr")?.trim())?;
            captures.get(1).or_else(|| captures.get(2))?.as_str()
        }
        _ => return None,
    };
    aggregated(src)
}

/// The text of a spec as Odoo sets it, blanks included, at its first non-blank character.
fn spec_raw(node: Node, lines: &Lines) -> Option<Spanned<String>> {
    let text = node.first_child().filter(Node::is_text)?;
    let raw = text.text()?;
    Some(Spanned {
        value: raw.to_owned(),
        pos: lines.pos(text.range().start + (raw.len() - raw.trim_start().len())),
    })
}

/// The text of a spec, trimmed, at its first character.
fn spec_text(node: Node, lines: &Lines) -> Option<Spanned<String>> {
    let Spanned { value, pos } = spec_raw(node, lines)?;
    Some(Spanned {
        value: value.trim().to_owned(),
        pos,
    })
}

/// Where the byte `at` of `decoded`, the value of the node `text`, sits in the source.
fn code_pos(text: Node, decoded: &str, at: usize, lines: &Lines) -> Pos {
    lines.pos(source_offset(
        text.document().input_text(),
        text.range().start,
        decoded,
        at,
    ))
}

/// The offset in `input` of the byte `at` of `decoded`, a text whose source opens at `start`. The
/// parser keeps no offsets past that start, so this reads the source again. CDATA markers read
/// nothing, but none is skipped at the end of `decoded`, so an error there stays on the closing one.
/// A DOCTYPE entity has no known width, so the walk stops on its `&`. It compares no characters: a
/// text opening with such an entity starts inside the DOCTYPE, and the result then means nothing.
fn source_offset(input: &str, start: usize, decoded: &str, at: usize) -> usize {
    let (mut source, mut read, mut cdata) = (start, 0, false);
    loop {
        while read < decoded.len() {
            let marker = if cdata { CDATA_END } else { CDATA };
            if !input[source..].starts_with(marker) {
                break;
            }
            source += marker.len();
            cdata = !cdata;
        }
        if read >= at {
            return source;
        }
        let step = decoded
            .get(read..)
            .and_then(|rest| source_step(&input[source..], rest, cdata));
        let Some((source_width, decoded_width)) = step else {
            return source;
        };
        source += source_width;
        read += decoded_width;
    }
}

/// The bytes the next character takes in `source` and in `decoded`, in that order. None where the
/// source stops or holds an entity of unknown width.
fn source_step(source: &str, decoded: &str, cdata: bool) -> Option<(usize, usize)> {
    if source.starts_with("\r\n") {
        return Some((2, 1));
    }
    let width = source.chars().next()?.len_utf8();
    if cdata || !source.starts_with('&') {
        return Some((width, width));
    }
    let end = source.find(';')?;
    let name = &source[1..end];
    let known = name.starts_with('#') || PREDEFINED_ENTITIES.contains(&name);
    known.then_some((end + 1, decoded.chars().next()?.len_utf8()))
}

fn literal_target(value: &str) -> bool {
    !value.trim().is_empty() && !value.bytes().all(|b| b.is_ascii_digit()) && !value.contains(['{', '/'])
}

/// The tag of the element a spec locates: its own, or an xpath's last step's.
pub fn spec_tag<'a>(spec: Node<'a, '_>) -> Option<&'a str> {
    match spec.tag_name().name() {
        "xpath" => {
            let step = spec.attribute("expr")?.rsplit('/').next()?;
            let step = step.rsplit("::").next()?;
            step.split('[').next()
        }
        tag => Some(tag),
    }
}

/// Whether a union or an `or` makes a step of an xpath optional.
fn optional_steps(expr: &str) -> bool {
    expr.contains('|') || expr.contains(" or ")
}

fn step_element(step: &Captures) -> ArchElement {
    ArchElement {
        tag: step[1].to_owned(),
        attr: step[2].to_owned(),
        value: step[3].to_owned(),
    }
}

/// The named steps of an xpath, each of which must match some element, so an expression with
/// optional steps is not checked.
fn xpath_locators(expr: &str, pos: Pos) -> Vec<Spanned<ArchElement>> {
    if optional_steps(expr) {
        return vec![];
    }
    let located = |c: Captures| Spanned {
        value: step_element(&c),
        pos,
    };
    XPATH_NAMED_STEP.captures_iter(expr).map(located).collect()
}

/// The element an xpath lands on, when its last step names it. Anything after that step, such as
/// `/..` or a further predicate, lands elsewhere or on only some of the named elements.
fn xpath_target(expr: &str) -> Option<ArchElement> {
    if optional_steps(expr) {
        return None;
    }
    let step = XPATH_NAMED_STEP.captures_iter(expr).last()?;
    (step.get(0)?.end() == expr.len()).then(|| step_element(&step))
}

fn subview(path: &[String], tag: &str) -> ArchSubview {
    ArchSubview {
        path: path.to_vec(),
        tag: tag.to_owned(),
        attribute_fields: vec![],
    }
}

/// What `_validate_tag_calendar` adds to the view's fields (`odoo/addons/base/models/ir_ui_view.py:1415`
/// on 17.0). No other subview loads a field without a `<field>`.
fn attribute_fields(node: Node) -> Vec<String> {
    if !node.has_tag_name("calendar") {
        return vec![];
    }
    let attributes = CALENDAR_FIELD_ATTRS
        .iter()
        .filter_map(|attr| node.attribute(*attr)?.split('.').next());
    let filters = node
        .children()
        .filter(|c| c.has_tag_name("filter"))
        .filter_map(|c| c.attribute("name"));
    attributes
        .chain(filters)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The path of the subview `node` opens when a spec puts it inside the field `path` locates. Odoo
/// reads only a field's direct children on the comodel, so a subview beside the field or wrapped in
/// another element stays on the model around it. Beside a subview an xpath locates, `path` is
/// already inside the field.
fn spec_subview(node: Node, path: &[String]) -> Option<Vec<String>> {
    if !SUBVIEWS.contains(&node.tag_name().name()) {
        return None;
    }
    let spec = node.parent_element()?;
    let Some(field) = path.last()?.strip_prefix(LOCATOR) else {
        return beside_subview(spec).then(|| path.to_vec());
    };
    let is_spec = spec.has_tag_name("xpath") || spec.has_attribute("position");
    let inside = matches!(spec.attribute("position"), None | Some("inside"));
    let opened = path.iter().map(String::as_str).chain([field]);
    (is_spec && inside).then(|| opened.map(str::to_owned).collect())
}

/// Whether `spec` is an xpath that puts its content where a subview of a field sits, or next to it.
fn beside_subview(spec: Node) -> bool {
    let target = spec
        .attribute("expr")
        .filter(|_| spec.has_tag_name("xpath"))
        .and_then(|expr| XPATH_LAST_SUBVIEW.captures(expr.trim()));
    matches!(spec.attribute("position"), Some("before" | "after" | "replace"))
        && target.is_some_and(|c| SUBVIEWS.contains(&&c[1]))
}

/// Extends `path` with where an xpath's target sits: the subviews it reaches into and the field it
/// locates. `around` gets those it names by tag, since only the xpath tells them apart when a field
/// holds more than one.
fn xpath_steps(expr: &str, path: &mut Vec<String>, around: &mut Vec<ArchSubview>) {
    let before = path.len();
    // Axes such as `/..` or `/following-sibling::x` stay in the field's own view.
    let containers = XPATH_CONTAINER
        .captures_iter(expr)
        .filter(|c| c.get(3).is_none() && &c[2] != "..");
    for c in containers {
        path.push(c[1].to_owned());
        if SUBVIEWS.contains(&&c[2]) {
            around.push(subview(path, &c[2]));
        }
    }
    if path.len() == before && XPATH_AMBIGUOUS.is_match(expr) {
        path.push(UNKNOWN_CONTEXT.to_owned());
    }
    if let Some(c) = XPATH_LAST_FIELD.captures(expr.trim()) {
        path.push(format!("{LOCATOR}{}", &c[1]));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn analyze(text: &str) -> DataFacts {
        super::analyze(text, None)
    }

    /// What the first field a record with the given field elements keeps ends on.
    fn write_of(fields: &str) -> String {
        let facts = analyze(&format!("<odoo><record id=\"r\" model=\"m\">{fields}</record></odoo>"));
        match &facts.writes[0].fields[0].value {
            WrittenValue::Ids(ids) => ids.iter().map(|i| i.value.as_str()).collect::<Vec<_>>().join(","),
            WrittenValue::Text(text) => format!("text:{}", text.value),
            WrittenValue::Model(model) => format!("model:{}", model.value),
            WrittenValue::Opaque => "opaque".to_owned(),
        }
    }

    fn first_write(data: &str) -> RecordWrite {
        analyze(&format!("<odoo>{data}</odoo>")).writes.swap_remove(0)
    }

    #[test]
    fn a_diagram_keeps_its_fields_as_elements_to_locate_and_none_to_check() {
        let arch = r#"<diagram>
            <node object="n"><field name="kind"/></node>
            <arrow object="a"><field name="signal"/></arrow>
            <field name="direct"/>
        </diagram>"#;
        let facts = analyze(&format!(
            r#"<odoo><record id="v" model="ir.ui.view"><field name="arch" type="xml">{arch}</field></record></odoo>"#
        ));
        let view = &facts.views[0];
        assert!(view.fields.is_empty());
        let located = view.elements.iter().map(|e| e.value.as_str());
        assert_eq!(located.collect::<Vec<_>>(), ["kind", "signal", "direct"]);
    }

    #[test]
    fn a_search_of_ir_model_by_name_writes_that_model() {
        let search = r#"search="[('model', '=', 'res.partner')]""#;
        assert_eq!(write_of(&format!("<field name=\"f\" {search}/>")), "model:res.partner");
        assert_eq!(
            write_of(&format!("<field name=\"f\" model=\"ir.model\" {search}/>")),
            "model:res.partner"
        );
        assert_eq!(
            write_of(&format!("<field name=\"f\" model=\"res.users\" {search}/>")),
            "opaque"
        );
        assert_eq!(write_of(&format!("<field name=\"f\" use=\"id\" {search}/>")), "opaque");
        assert_eq!(write_of(r#"<field name="f" search="[('name', '=', 'x')]"/>"#), "opaque");
    }

    #[test]
    fn places_a_write_at_its_record() {
        let write = first_write("\n  <record id=\"r\" model=\"m\"/>");
        assert_eq!(write.pos, Pos { line: 2, col: 3 });
    }

    #[test]
    fn a_later_field_of_a_name_decides_whether_it_adds() {
        let incremental = |fields: &str| {
            let write = first_write(&format!("<record id=\"r\" model=\"m\">{fields}</record>"));
            write.fields[0].incremental
        };
        assert!(incremental(r#"<field name="f" eval="[(4, ref('a'))]"/>"#));
        assert!(incremental(r#"<field name="f" ref="" eval="[(0, 0, {})]"/>"#));
        assert!(incremental(r#"<field name="f"><record model="n"/></field>"#));
        assert!(incremental(
            r#"<field name="f" ref="a"/><field name="f" eval="[(3, 1)]"/>"#
        ));
        assert!(!incremental(
            r#"<field name="f" eval="[(3, 1)]"/><field name="f" ref="a"/>"#
        ));
        assert!(!incremental(r#"<field name="f" eval="[(6, 0, [])]"/>"#));
        assert!(!incremental(r#"<field name="f" ref="a" eval="[(4, 1)]"/>"#));
        assert!(!incremental(r#"<field name="f" search="[]" eval="[(4, 1)]"/>"#));
        assert!(!incremental(r#"<field name="f">[(4, 1)]</field>"#));
    }

    #[test]
    fn reads_the_context_of_a_record_and_of_its_roots() {
        let text = r#"<odoo context="{'default_a': 1}"><data context="{'default_b': 1}">
            <record model="m" context="{'default_c': 1, 'other': 2}">
                <field name="f"><record model="n" context="{'default_d': ref('x')}"/></field>
            </record>
        </data></odoo>"#;
        let facts = analyze(text);
        assert_eq!(facts.writes[0].context_defaults, ["c", "b", "a"]);
        assert_eq!(facts.writes[1].context_defaults, ["d", "b", "a"]);
        assert!(first_write(r#"<record model="m"/>"#).context_defaults.is_empty());
    }

    #[test]
    fn reads_forcecreate_as_odoo_does() {
        let no_create = |attr: &str| first_write(&format!("<record model=\"m\" {attr}/>")).no_create;
        for off in ["0", "False", "false", " OFF "] {
            assert!(no_create(&format!("forcecreate=\"{off}\"")), "{off}");
        }
        for on in ["", " ", "1", "True", "x"] {
            assert!(!no_create(&format!("forcecreate=\"{on}\"")), "{on}");
        }
        assert!(!no_create(""));
    }

    #[test]
    fn reads_the_field_of_an_ir_default() {
        let default_of = |function: &str| {
            let facts = analyze(&format!("<odoo>{function}</odoo>"));
            facts.functions[0].default_of.clone()
        };
        let found = Some(("a.b".to_owned(), "c".to_owned()));
        let set = r#"model="ir.default" name="set""#;
        assert_eq!(
            default_of(&format!(r#"<function {set} eval="('a.b', 'c', ref('d'))"/>"#)),
            found
        );
        assert_eq!(default_of(&format!(r#"<function {set} eval="['a.b', 'c']"/>"#)), found);
        assert_eq!(
            default_of(&format!(
                r#"<function {set}><value>a.b</value><value>c</value><value name="user_id">1</value></function>"#
            )),
            found
        );
        assert_eq!(
            default_of(&format!(
                r#"<function {set} eval="('a.b',)"><value>c</value></function>"#
            )),
            found
        );
        assert_eq!(default_of(&format!(r#"<function {set} eval="(name, 'c')"/>"#)), None);
        assert_eq!(default_of(&format!(r#"<function {set} eval="('a.b',)"/>"#)), None);
        assert_eq!(
            default_of(r#"<function model="ir.default" name="other" eval="('a.b', 'c')"/>"#),
            None
        );
        assert_eq!(
            default_of(r#"<function model="x" name="set" eval="('a.b', 'c')"/>"#),
            None
        );
    }

    #[test]
    fn reads_a_written_value_in_the_order_of_odoo() {
        assert_eq!(write_of(r#"<field name="f" search="[]" ref="a"/>"#), "opaque");
        assert_eq!(
            write_of(r#"<field name="f" search="" ref="a" eval="ref('b')">c</field>"#),
            "a"
        );
        assert_eq!(write_of(r#"<field name="f" ref="" eval="ref('b')">c</field>"#), "b");
        assert_eq!(write_of(r#"<field name="f" eval="" type="char">c</field>"#), "opaque");
        assert_eq!(write_of(r#"<field name="f" eval="" >c</field>"#), "text:c");
        assert_eq!(write_of(r#"<field name="f"> c </field>"#), "text: c ");
    }

    #[test]
    fn takes_the_values_only_a_run_tells_as_opaque() {
        assert_eq!(write_of(r#"<field name="f"/>"#), "opaque");
        assert_eq!(write_of(r#"<field name="f"><!-- c -->d</field>"#), "opaque");
        assert_eq!(write_of(r#"<field name="f" type="int">1</field>"#), "opaque");
        assert_eq!(write_of(r#"<field name="f" file="a.txt">c</field>"#), "opaque");
        assert_eq!(write_of(r#"<field name="f" eval="1 + 1"/>"#), "opaque");
    }

    #[test]
    fn writes_the_last_field_of_a_name() {
        assert_eq!(write_of(r#"<field name="f" ref="a"/><field name="f" ref="b"/>"#), "b");
        assert_eq!(
            write_of(r#"<field name="f" ref="a"/><field name="f" eval="False"/>"#),
            "opaque"
        );
    }

    #[test]
    fn writes_a_nested_record_after_the_one_that_holds_it() {
        let facts = analyze(
            r#"<odoo><record id="p" model="m"><field name="line_ids">
                <record id="c" model="n"><field name="x" ref="a"/></record>
            </field></record></odoo>"#,
        );
        assert_eq!(facts.writes[0].id.as_deref(), Some("p"));
        assert_eq!(facts.writes[0].nested_in, None);
        assert_eq!(facts.writes[1].id.as_deref(), Some("c"));
        assert_eq!(facts.writes[1].nested_in, Some(("m".to_owned(), "line_ids".to_owned())));
    }

    #[test]
    fn gives_each_shorthand_its_model() {
        let facts = analyze(
            r#"<odoo><template id="t"><div/></template><menuitem id="m"><menuitem id="n"/></menuitem>
            <act_window id="w" name="w" res_model="x"/><report id="p" model="x" name="n" string="s"/>
            <asset id="a"/><record id="r" model="x"/></odoo>"#,
        );
        let def = |d: &XmlDef| (d.id.clone(), d.model.clone().unwrap_or_default(), d.themed);
        let found: Vec<_> = facts.defs.iter().map(def).collect();
        let expected = [
            ("t", "ir.ui.view", true),
            ("m", "ir.ui.menu", false),
            ("n", "ir.ui.menu", false),
            ("w", "ir.actions.act_window", false),
            ("p", "ir.actions.report", false),
            ("a", "ir.asset", true),
            ("r", "x", false),
        ];
        assert_eq!(found, expected.map(|(i, m, t)| (i.to_owned(), m.to_owned(), t)));
    }

    fn calls(arch: &str) -> Vec<(String, u32, u32)> {
        let facts = analyze(&format!("<odoo>\n<template id=\"x\">{arch}</template>\n</odoo>"));
        let view = &facts.views[0];
        let pos = |c: &Spanned<String>| (c.value.clone(), c.pos.line, c.pos.col);
        view.calls.iter().map(pos).collect()
    }

    #[test]
    fn reads_a_literal_t_call_at_its_value() {
        assert_eq!(calls(r#"<t t-call="web.layout"/>"#), [("web.layout".to_owned(), 2, 29)]);
        assert_eq!(calls(r#"<t t-call="local"/>"#), [("local".to_owned(), 2, 29)]);
    }

    #[test]
    fn reads_an_attribute_spec_at_its_text() {
        let spec =
            "<xpath expr=\"//t\" position=\"attributes\"><attribute name=\"t-call\">\n  a.b\n</attribute></xpath>";
        assert_eq!(calls(spec), [("a.b".to_owned(), 3, 3)]);
    }

    #[test]
    fn skips_targets_only_a_render_can_tell() {
        for target in ["", "  ", "33", "{{ x }}", "a.#{x}", "a/templates/b.xml"] {
            assert_eq!(calls(&format!(r#"<t t-call="{target}"/>"#)), [], "{target}");
        }
        assert_eq!(calls(r#"<attribute name="t-call"> </attribute>"#), []);
    }

    fn view(data: &str) -> ViewFact {
        analyze(&format!("<odoo>{data}</odoo>")).views.swap_remove(0)
    }

    fn call_names(view: &ViewFact) -> Vec<&str> {
        view.calls.iter().map(|c| c.value.as_str()).collect()
    }

    fn located_values(view: &ViewFact) -> Vec<String> {
        let shown = |l: &Spanned<ArchElement>| format!("{}[{}={}]", l.value.tag, l.value.attr, l.value.value);
        view.locators.iter().map(shown).collect()
    }

    fn extension(arch: &str) -> ViewFact {
        view(&format!(r#"<template id="x" inherit_id="y">{arch}</template>"#))
    }

    #[test]
    fn a_spec_locates_a_call_and_its_content_calls() {
        let spec = extension(r#"<t t-call="a.old" position="replace"><t t-call="a.new" position="'top'"/></t>"#);
        assert_eq!(call_names(&spec), ["a.new"]);
        assert_eq!(located_values(&spec), ["t[t-call=a.old]"]);
    }

    #[test]
    fn a_spec_needs_no_position() {
        let spec = extension(r#"<t t-call="a.old"/><div name="b"/>"#);
        assert_eq!(call_names(&spec), [""; 0]);
        assert_eq!(located_values(&spec), ["t[t-call=a.old]", "div[name=b]"]);
    }

    #[test]
    fn an_attribute_of_a_spec_on_a_call_still_calls() {
        let spec =
            extension(r#"<t t-call="a.old" position="attributes"><attribute name="t-call">a.new</attribute></t>"#);
        assert_eq!(call_names(&spec), ["a.new"]);
        assert!(spec.elements.iter().any(|e| e.attr == "t-call" && e.value == "a.new"));
        let unwrapped = extension(r#"<attribute name="t-call">a.new</attribute>"#);
        assert_eq!(call_names(&unwrapped), ["a.new"]);
    }

    #[test]
    fn a_moved_call_locates_and_a_deeper_one_is_content() {
        let moved = r#"<xpath expr="//a" position="after"><t t-call="a.moved" position="move"/></xpath>"#;
        let spec = extension(moved);
        assert_eq!(call_names(&spec), [""; 0]);
        assert_eq!(located_values(&spec), ["t[t-call=a.moved]"]);
        let deeper = r#"<xpath expr="//a"><div><t t-call="a.kept" position="move"/></div></xpath>"#;
        assert_eq!(call_names(&extension(deeper)), ["a.kept"]);
    }

    /// The assets of a template as `directive:target>path`.
    fn assets(view: &ViewFact) -> Vec<String> {
        let shown = |a: &AssetEntry| format!("{:?}:{}>{}", a.directive, a.target.as_deref().unwrap_or(""), a.path);
        view.assets.iter().map(shown).collect()
    }

    #[test]
    fn a_root_template_lists_its_aggregated_scripts_and_calls_in_order() {
        let root = view(
            r#"<template id="x"><t t-call="web._helpers"/><script src="/a/static/src/a.js"/>
            <script src="https://cdn.example.com/x.js"/><script src="//cdn.example.com/y.js"/>
            <script src="/web/content/12"/><script src="a/static/src/b.js"/><script/><script src="data:x"/>
            <t t-if="debug"><script src="/a//static/src/d.js"/></t><script src="///a/static/src/e.js"/>
            <div><script type="text/javascript" src="/a/static/src/c.js"/></div><t t-call="{{ name }}"/>
            <t t-call="a.layout"><script src="/a/static/src/body.js"/></t></template>"#,
        );
        assert_eq!(
            assets(&root),
            [
                "Include:>web._helpers",
                "Append:>a/static/src/a.js",
                "Append:>a/static/src/b.js",
                "Append:>a/static/src/d.js",
                "Append:>a/static/src/e.js",
                "Include:>a.layout",
            ]
        );
        let called = view(r#"<template id="x" t-call="web.layout"/>"#);
        assert_eq!(assets(&called), ["Include:>web.layout"]);
    }

    #[test]
    fn an_extension_places_its_scripts_against_the_script_a_spec_locates() {
        let spec = extension(
            r#"<xpath expr="." position="inside"><script src="/a/static/src/in.js"/><t t-call="a.more"/></xpath>
            <xpath expr="//script[@src='/web/static/src/x.js']" position="after">
                <script src="/a/static/src/one.js"/><script src="/a/static/src/two.js"/>
            </xpath>
            <data><script src="/web/static/src/y.js" position="before"><script src="/a/static/src/b.js"/></script></data>
            <xpath expr="//script[@src=&quot;/web/static/src/z.js&quot;]" position="replace">
                <script src="/a/static/src/z.js"/>
            </xpath>
            <xpath expr="//script[@src='/web/static/src/gone.js']" position="replace"/>
            <xpath expr="//script[last()]" position="after"><script src="/a/static/src/last.js"/></xpath>"#,
        );
        assert_eq!(
            assets(&spec),
            [
                "Append:>a/static/src/in.js",
                "Include:>a.more",
                "After:web/static/src/x.js>a/static/src/one.js",
                "After:a/static/src/one.js>a/static/src/two.js",
                "Before:web/static/src/y.js>a/static/src/b.js",
                "Replace:web/static/src/z.js>a/static/src/z.js",
                "Remove:>web/static/src/gone.js",
                "Append:>a/static/src/last.js",
            ]
        );
    }

    #[test]
    fn a_located_script_or_call_adds_nothing() {
        let spec = extension(
            r#"<t t-call="web._helpers" position="replace"/><script src="/web/static/src/x.js" position="attributes">
            <attribute name="src">/a/static/src/x.js</attribute></script>"#,
        );
        assert!(spec.assets.is_empty());
    }

    #[test]
    fn a_position_in_a_plain_template_is_a_parameter() {
        let plain = view(r#"<template id="x"><t t-call="a.b" position="'top'"/></template>"#);
        assert_eq!(call_names(&plain), ["a.b"]);
        assert!(plain.locators.is_empty());
        let unset = view(r#"<template id="x" inherit_id=""><t t-call="a.b"/></template>"#);
        assert_eq!(call_names(&unset), ["a.b"]);
    }

    #[test]
    fn a_data_wrapper_lends_its_children_whatever_the_order_of_the_fields() {
        for field in ["arch", "arch_db"] {
            let arch = format!(
                r#"<field name="{field}" type="xml"><data><data><t t-call="a.old" position="replace"/></data></data></field>"#
            );
            let spec = view(&format!(
                r#"<record id="x" model="ir.ui.view">{arch}<field name="inherit_id" ref="y"/></record>"#
            ));
            assert_eq!(call_names(&spec), [""; 0]);
            assert_eq!(located_values(&spec), ["t[t-call=a.old]"]);
        }
    }

    #[test]
    fn the_root_of_an_extending_arch_is_a_spec() {
        let spec = view(
            r#"<record id="x" model="ir.ui.view"><field name="inherit_id" ref="y"/><field name="arch" type="xml"><t t-call="a.old"/></field></record>"#,
        );
        assert_eq!(call_names(&spec), [""; 0]);
        assert_eq!(located_values(&spec), ["t[t-call=a.old]"]);
    }

    #[test]
    fn a_record_without_a_parent_holds_no_specs() {
        for unset in [r#"eval="False""#, r#"eval="None""#, r#"ref="""#, ""] {
            let inherit = format!(r#"<field name="inherit_id" {unset}/>"#);
            let arch = r#"<field name="arch" type="xml"><t t-call="a.b"/></field>"#;
            for fields in [format!("{inherit}{arch}"), format!("{arch}{inherit}")] {
                let root = view(&format!(r#"<record id="x" model="ir.ui.view">{fields}</record>"#));
                assert_eq!(call_names(&root), ["a.b"]);
                assert!(root.locators.is_empty());
                assert!(root.elements.iter().any(|e| e.attr == "t-call" && e.value == "a.b"));
                assert_eq!(root.inherit, None);
            }
        }
    }

    #[test]
    fn a_record_with_a_parent_holds_specs_however_it_writes_it() {
        for inherit in [
            r#"<field name="inherit_id" eval="ref('y')"/>"#,
            r#"<field name="inherit_id" ref="" eval="ref('y')"/>"#,
            r#"<field name="inherit_id" model="ir.ui.view" search="[('key', '=', 'y')]"/>"#,
            r#"<field name="inherit_id">12</field>"#,
        ] {
            let spec = view(&format!(
                r#"<record id="x" model="ir.ui.view">{inherit}<field name="arch" type="xml"><t t-call="a.old"/></field></record>"#
            ));
            assert_eq!(call_names(&spec), [""; 0]);
            assert_eq!(located_values(&spec), ["t[t-call=a.old]"]);
        }
    }

    #[test]
    fn a_field_of_a_view_named_arch_holds_no_specs() {
        let form = view(
            r#"<record id="x" model="ir.ui.view"><field name="arch" type="xml"><form><field name="inherit_id"/><field name="arch"><t t-call="a.b"/></field></form></field></record>"#,
        );
        assert_eq!(call_names(&form), ["a.b"]);
    }

    #[test]
    fn an_xpath_step_locates_a_call() {
        let spec = extension(r#"<xpath expr="//t[@t-call='a.old']/div[@id='b']" position="inside"/>"#);
        assert_eq!(located_values(&spec), ["t[t-call=a.old]", "div[id=b]"]);
    }

    #[test]
    fn a_spec_can_locate_the_call_of_a_template_root() {
        let found = |attrs: &str| {
            let facts = view(&format!(r#"<template id="x" {attrs} t-call="a.b"/>"#));
            facts
                .elements
                .iter()
                .any(|e| (e.tag.as_str(), e.attr.as_str(), e.value.as_str()) == ("t", "t-call", "a.b"))
        };
        assert!(found(""));
        assert!(found(r#"inherit_id="""#));
        assert!(!found(r#"inherit_id="y""#));
    }

    #[test]
    fn a_template_root_calls_too() {
        let facts = analyze(r#"<odoo><template id="x" t-call="a.b"/></odoo>"#);
        assert_eq!(facts.views[0].calls[0].value, "a.b");
    }

    #[test]
    fn an_extending_template_root_calls_nothing() {
        let calls = |attrs: &str| {
            let facts = analyze(&format!(
                r#"<odoo><template id="x" {attrs} t-call="a.root"><xpath expr="//a"><t t-call="a.inner"/></xpath></template></odoo>"#
            ));
            facts.views[0].calls.iter().map(|c| c.value.clone()).collect::<Vec<_>>()
        };
        assert_eq!(calls(r#"inherit_id="y""#), ["a.inner"]);
        assert_eq!(calls(r#"inherit_id="y" primary="True""#), ["a.inner"]);
        assert_eq!(calls(r#"inherit_id="""#), ["a.root", "a.inner"]);
    }

    fn buttons(arch: &str) -> Vec<(Vec<String>, String, u32, u32)> {
        let facts = analyze(&format!(
            "<odoo><record id=\"x\" model=\"ir.ui.view\">\n<field name=\"arch\" type=\"xml\">{arch}</field></record></odoo>"
        ));
        let view = &facts.views[0];
        let read = |b: &ArchButton| {
            (
                b.path.clone(),
                b.method.value.clone(),
                b.method.pos.line,
                b.method.pos.col,
            )
        };
        view.buttons.iter().map(read).collect()
    }

    #[test]
    fn reads_an_object_button_at_its_name_with_its_arch_position() {
        let nested = r#"<form><field name="line_ids"><list><button type="object" name="a"/></list></field></form>"#;
        assert_eq!(buttons(nested), [(vec!["line_ids".to_owned()], "a".to_owned(), 2, 94)]);
        let grouped = r#"<list><groupby name="b"><button type="object" name="c"/></groupby></list>"#;
        assert_eq!(buttons(grouped)[0].0, ["b"]);
        let inserted = r#"<xpath expr="//header" position="inside"><button type="object" name="d"/></xpath>"#;
        assert_eq!(buttons(inserted)[0].1, "d");
    }

    #[test]
    fn skips_buttons_that_call_no_literal_method() {
        for button in [
            r#"<button name="a"/>"#,
            r#"<button type="action" name="a"/>"#,
            r#"<button type="object" name="a" special="cancel"/>"#,
            r#"<button type="object" name="a" position="after"/>"#,
            r#"<button type="object" t-att-name="a"/>"#,
            r#"<button type="object" name="%(a)d"/>"#,
            r#"<button type="object" name="{{ a }}"/>"#,
            r#"<button type="object" name=""/>"#,
            r#"<a type="object" name="a"/>"#,
        ] {
            assert_eq!(buttons(&format!("<form>{button}</form>")), [], "{button}");
        }
    }

    fn modifiers(arch: &str) -> Vec<(Vec<String>, &'static str, String, u32, u32)> {
        let facts = analyze(&format!(
            "<odoo><record id=\"x\" model=\"ir.ui.view\">\n<field name=\"arch\" type=\"xml\">{arch}</field></record></odoo>"
        ));
        let read = |m: &ArchModifier| {
            let pos = m.value.pos;
            (m.path.clone(), m.attr, m.value.value.clone(), pos.line, pos.col)
        };
        facts.views[0].modifiers.iter().map(read).collect()
    }

    fn values(arch: &str) -> Vec<String> {
        modifiers(arch).into_iter().map(|m| m.2).collect()
    }

    #[test]
    fn reads_each_modifier_at_its_value_and_arch_position() {
        let arch =
            r#"<form><field name="l"><list><field name="a" column_invisible="b" required="c"/></list></field></form>"#;
        let found = modifiers(arch);
        assert_eq!(
            found[0],
            (vec!["l".to_owned()], "column_invisible", "b".to_owned(), 2, 93)
        );
        assert_eq!(found[1].1, "required");
        let grouped = r#"<list><groupby name="g"><button readonly="a"/></groupby></list>"#;
        assert_eq!(modifiers(grouped)[0].0, ["g"]);
    }

    #[test]
    fn skips_the_values_odoo_takes_before_it_parses() {
        for value in ["", "0", "1", "true", "false", "TRUE", "False"] {
            assert_eq!(values(&format!(r#"<form invisible="{value}"/>"#)), [""; 0], "{value}");
        }
        assert_eq!(values(r#"<form invisible=" "/>"#), [" "]);
    }

    #[test]
    fn a_spec_text_keeps_the_blanks_that_make_it_no_constant() {
        let spec = |text: &str| values(&format!(r#"<attribute name="readonly">{text}</attribute>"#));
        assert_eq!(spec(" "), [" "]);
        assert_eq!(spec("\n  true\n"), ["\n  true\n"]);
        assert_eq!(spec("true"), [""; 0]);
        assert_eq!(spec(""), [""; 0]);
    }

    #[test]
    fn an_attribute_of_a_spec_locates_and_is_no_modifier() {
        assert_eq!(values(r#"<field name="a" invisible="b" position="after"/>"#), [""; 0]);
        assert_eq!(values(r#"<xpath expr="//a" invisible="b" position="after"/>"#), [""; 0]);
        let inserted = r#"<field name="a" position="after"><field name="b" invisible="c"/></field>"#;
        assert_eq!(values(inserted), ["c"]);
    }

    #[test]
    fn reads_an_attribute_spec_at_its_text_with_its_target() {
        let spec =
            "<field name=\"a\" position=\"attributes\"><attribute name=\"invisible\">\n  b\n</attribute></field>";
        assert_eq!(
            modifiers(spec),
            [(vec!["~a".to_owned()], "invisible", "\n  b\n".to_owned(), 3, 3)]
        );
        assert_eq!(values(r#"<attribute name="string">b</attribute>"#), [""; 0]);
    }

    #[test]
    fn reads_an_add_joined_by_and_or_or() {
        for separator in ["and", " or "] {
            let spec = format!(r#"<attribute name="invisible" add="b" separator="{separator}"/>"#);
            assert_eq!(values(&spec), ["b"]);
        }
        assert_eq!(values(r#"<attribute name="invisible" add="b"/>"#), [""; 0]);
        assert_eq!(
            values(r#"<attribute name="invisible" add="b" separator=","/>"#),
            [""; 0]
        );
        assert_eq!(
            values(r#"<attribute name="invisible" remove="b" separator="or"/>"#),
            [""; 0]
        );
        assert_eq!(
            values(r#"<attribute name="invisible" separator="or">b</attribute>"#),
            ["b"]
        );
    }

    #[test]
    fn an_add_joins_the_element_its_spec_locates() {
        let joined = |spec: &str| {
            let joins = arch_view(spec).modifiers.swap_remove(0).joins;
            joins.map(|e| (e.tag, e.attr, e.value))
        };
        let add = r#"<attribute name="invisible" add="true" separator="or"/>"#;
        let target = |tag: &str, attr: &str, value: &str| Some((tag.to_owned(), attr.to_owned(), value.to_owned()));
        let spec = format!(r#"<field name="a" position="attributes">{add}</field>"#);
        assert_eq!(joined(&spec), target("field", "name", "a"));
        let first = format!(r#"<div id="a" name="b" position="attributes">{add}</div>"#);
        assert_eq!(joined(&first), target("div", "name", "b"));
        for (expr, found) in [
            ("//field[@name='a']", target("field", "name", "a")),
            ("//group/div[@id='a']", target("div", "id", "a")),
            ("//field[@name='a']/field[@name='b']", target("field", "name", "b")),
        ] {
            let spec = format!(r#"<xpath expr="{expr}" position="attributes">{add}</xpath>"#);
            assert_eq!(joined(&spec), found, "{expr}");
        }
    }

    #[test]
    fn an_xpath_that_ends_on_no_named_step_joins_nothing() {
        for expr in [
            "//field[@name='a']/..",
            "//field[@name='a'][1]",
            "//group[field[@name='a']]",
            "//field[@name='a'] | //field[@name='b']",
            "//field[@name='a' or @name='b']",
            "//form",
        ] {
            let add = r#"<attribute name="invisible" add="b" separator="or"/>"#;
            let spec = format!(r#"<xpath expr="{expr}" position="attributes">{add}</xpath>"#);
            assert_eq!(arch_view(&spec).modifiers[0].joins, None, "{expr}");
        }
    }

    #[test]
    fn a_constant_add_is_kept_only_when_it_joins_an_element() {
        let spec = |attrs: &str| {
            let attribute = format!(r#"<attribute name="invisible" {attrs} separator="or"/>"#);
            arch_view(&format!(r#"<field name="a" position="attributes">{attribute}</field>"#)).modifiers
        };
        assert_eq!(spec(r#"add="true""#)[0].value.value, "true");
        assert!(spec(r#"add="true" remove="""#)[0].joins.is_some());
        assert!(spec(r#"add="true" remove="b""#).is_empty());
        assert!(spec(r#"add="""#).is_empty());
        assert_eq!(spec(r#"add="b" remove="c""#)[0].joins, None);
    }

    #[test]
    fn a_spec_that_may_empty_a_modifier_records_it() {
        let emptied = |attribute: &str| {
            let spec = extension(&format!(r#"<field name="a" position="attributes">{attribute}</field>"#));
            spec.emptied.iter().map(|e| e.attr).collect::<Vec<_>>()
        };
        for attribute in [
            r#"<attribute name="readonly"/>"#,
            r#"<attribute name="readonly"></attribute>"#,
            r#"<attribute name="readonly" add="" remove=""/>"#,
            r#"<attribute name="readonly" remove="b" separator="or"/>"#,
            r#"<attribute name="readonly" add="c" remove="b" separator="or"/>"#,
        ] {
            assert_eq!(emptied(attribute), ["readonly"], "{attribute}");
        }
        for attribute in [
            r#"<attribute name="readonly"> </attribute>"#,
            r#"<attribute name="readonly">b</attribute>"#,
            r#"<attribute name="readonly" add="b" separator="or"/>"#,
            r#"<attribute name="string"/>"#,
        ] {
            assert_eq!(emptied(attribute), [""; 0], "{attribute}");
        }
    }

    #[test]
    fn a_spec_that_empties_a_modifier_records_the_element_it_names() {
        let target = |spec: &str| {
            let emptied = extension(spec).emptied.swap_remove(0);
            emptied.target.map(|e| (e.tag, e.attr, e.value))
        };
        let named = |tag: &str, attr: &str, value: &str| Some((tag.to_owned(), attr.to_owned(), value.to_owned()));
        let clear = r#"<attribute name="readonly"/>"#;
        let field = format!(r#"<field name="a" position="attributes">{clear}</field>"#);
        assert_eq!(target(&field), named("field", "name", "a"));
        let xpath = format!(r#"<xpath expr="//div[@id='a']" position="attributes">{clear}</xpath>"#);
        assert_eq!(target(&xpath), named("div", "id", "a"));
        let parent = format!(r#"<xpath expr="//field[@name='a']/.." position="attributes">{clear}</xpath>"#);
        assert_eq!(target(&parent), None);
        let form = format!(r#"<form position="attributes">{clear}</form>"#);
        assert_eq!(target(&form), None);
    }

    #[test]
    fn records_each_named_element_with_the_modifiers_it_holds() {
        let arch =
            r#"<form><field name="a" readonly="b" invisible="" required="0"/><div id="c" name="d"/><group/></form>"#;
        let read = |v: ArchValued| {
            let names = v.elements.into_iter().map(|e| (e.attr, e.value)).collect();
            (names, v.attrs)
        };
        let valued: Vec<_> = arch_view(arch).valued.into_iter().map(read).collect();
        let held = |names: &[(&str, &str)], attrs: &[&'static str]| {
            let names: Vec<_> = names.iter().map(|&(a, v)| (a.to_owned(), v.to_owned())).collect();
            (names, attrs.to_vec())
        };
        assert_eq!(
            valued,
            [
                held(&[("name", "a")], &["readonly", "required"]),
                held(&[("name", "d"), ("id", "c")], &[]),
            ]
        );
    }

    #[test]
    fn a_spec_adds_no_valued_element_but_its_content_does() {
        let spec = r#"<field name="a" position="after" readonly="b"><field name="c" readonly="d"/></field>"#;
        let added = extension(spec).valued;
        assert_eq!(added.len(), 1);
        assert_eq!(added[0].elements[0].value, "c");
    }

    fn arch_view(arch: &str) -> ViewFact {
        view(&format!(
            r#"<record id="x" model="ir.ui.view"><field name="arch" type="xml">{arch}</field></record>"#
        ))
    }

    fn subviews(arch: &str) -> Vec<Vec<String>> {
        arch_view(arch).subviews.into_iter().map(|s| s.path).collect()
    }

    #[test]
    fn records_the_tag_of_each_subview() {
        let tags = |arch| arch_view(arch).subviews.into_iter().map(|s| s.tag).collect::<Vec<_>>();
        assert_eq!(
            tags(r#"<form><field name="a"><calendar/></field></form>"#),
            ["calendar"]
        );
        assert_eq!(tags(r#"<field name="a" position="inside"><graph/></field>"#), ["graph"]);
        assert_eq!(tags(r#"<tree><groupby name="a"/></tree>"#), ["groupby"]);
    }

    #[test]
    fn a_calendar_records_the_fields_it_adds() {
        let fields = |arch| arch_view(arch).subviews.swap_remove(0).attribute_fields;
        let calendar = r#"<form><field name="a"><calendar date_start="s" date_delay="d" date_stop="e" color="c.color"
            all_day="y" mode="month"><filter name="f"/><field name="n"/><group><filter name="deep"/></group></calendar></field></form>"#;
        assert_eq!(fields(calendar), ["s", "d", "e", "c", "y", "f"]);
        let unset = r#"<form><field name="a"><calendar date_stop=""><filter/></calendar></field></form>"#;
        assert!(fields(unset).is_empty());
        let other = r#"<form><field name="a"><graph date_start="s"><filter name="f"/></graph></field></form>"#;
        assert!(fields(other).is_empty());
    }

    #[test]
    fn an_xpath_on_a_subview_opens_what_it_puts_beside_it() {
        let spec =
            |position| format!(r#"<xpath expr="//field[@name='a']/tree" position="{position}"><calendar/></xpath>"#);
        for position in ["before", "after", "replace"] {
            assert_eq!(subviews(&spec(position)), [vec!["a"]]);
        }
        assert!(subviews(&spec("inside")).is_empty());
        let nested =
            r#"<xpath expr="//field[@name='a']/form//field[@name='b']/tree[1]" position="after"><form/></xpath>"#;
        assert_eq!(subviews(nested), [vec!["a", "b"]]);
        let through = r#"<xpath expr="//field[@name='a']/form/group" position="after"><form/></xpath>"#;
        assert!(subviews(through).is_empty());
        let sibling = r#"<xpath expr="//field[@name='a']/following-sibling::tree" position="after"><form/></xpath>"#;
        assert!(subviews(sibling).is_empty());
    }

    #[test]
    fn a_modifier_keeps_the_subviews_around_it() {
        let around = |arch: &str| {
            let subviews = arch_view(arch).modifiers.swap_remove(0).subviews;
            subviews.into_iter().map(|s| s.tag).collect::<Vec<_>>()
        };
        let inline = r#"<form><field name="a"><calendar/><tree><field name="b" invisible="c"/></tree></field></form>"#;
        assert_eq!(around(inline), ["tree"]);
        let reached = r#"<xpath expr="//field[@name='a']/calendar/field[@name='b']" position="after"><field name="c" invisible="d"/></xpath>"#;
        assert_eq!(around(reached), ["calendar"]);
        let swapped = r#"<xpath expr="//field[@name='a']/calendar" position="replace"><tree><field name="c" invisible="d"/></tree></xpath>"#;
        assert_eq!(around(swapped), ["calendar", "tree"]);
        let unnamed = r#"<xpath expr="//field[@name='a']//field[@name='b']" position="after"><field name="c" invisible="d"/></xpath>"#;
        assert!(around(unnamed).is_empty());
    }

    #[test]
    fn records_where_subviews_open() {
        let arch = r#"<form><field name="a"><list><field name="b"><form/></field></list><form/></field></form>"#;
        assert_eq!(subviews(arch), [vec!["a"], vec!["a", "b"], vec!["a"]]);
    }

    #[test]
    fn records_a_subview_a_spec_adds_past_its_locator() {
        let spec = r#"<field name="a" position="inside"><list/></field>"#;
        assert_eq!(subviews(spec), [vec!["~a", "a"]]);
        let xpath = r#"<xpath expr="//field[@name='b']//field[@name='a']" position="inside"><form/></xpath>"#;
        assert_eq!(subviews(xpath), [vec!["b", "~a", "a"]]);
        let bare = r#"<xpath expr="//field[@name='a']"><form/></xpath>"#;
        assert_eq!(subviews(bare), [vec!["~a", "a"]]);
        assert_eq!(
            subviews(r#"<field name="a" position="after"><field name="b"/></field>"#),
            [[""; 0]; 0]
        );
        assert_eq!(
            subviews(r#"<field name="a" position="after"><list/></field>"#),
            [[""; 0]; 0]
        );
        assert_eq!(
            subviews(r#"<field name="a" position="inside"><group><list/></group></field>"#),
            [[""; 0]; 0]
        );
        let nested = r#"<field name="a" position="inside"><list><field name="b"><form/></field></list></field>"#;
        assert_eq!(subviews(nested), [vec!["~a", "a"], vec!["~a", "a", "b"]]);
    }

    #[test]
    fn reads_a_function_at_its_name() {
        let facts = analyze(
            "<odoo><data>\n<function model=\"m\" name=\"a\"/>\n<function name=\"b\"/>\n<function model=\"m\"/>\n</data></odoo>",
        );
        let found: Vec<_> = facts
            .functions
            .iter()
            .map(|f| {
                (
                    f.model.as_str(),
                    f.method.value.as_str(),
                    f.method.pos.line,
                    f.method.pos.col,
                )
            })
            .collect();
        assert_eq!(found, [("m", "a", 2, 27)]);
    }

    fn action_facts(records: &str) -> Vec<ActionFact> {
        analyze(&format!("<odoo>{records}</odoo>")).actions
    }

    fn actions(records: &str) -> Vec<(Option<String>, Option<String>)> {
        let value = |v: &ActionValue| match v {
            ActionValue::Literal(s) => Some(s.value.clone()),
            _ => None,
        };
        let read = |a: &ActionFact| (value(&a.model), value(&a.view_mode));
        action_facts(records).iter().map(read).collect()
    }

    fn action_record(fields: &str) -> String {
        format!(r#"<record id="a" model="ir.actions.act_window">{fields}</record>"#)
    }

    fn literal_of(value: &ActionValue) -> &Spanned<String> {
        match value {
            ActionValue::Literal(s) => s,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn reads_an_action_record_at_its_text() {
        let facts = analyze(&format!(
            "<odoo>\n{}</odoo>",
            action_record("<field name=\"res_model\">\n  a.b\n</field><field name=\"view_mode\">list,gantt</field>")
        ));
        let action = &facts.actions[0];
        let model = literal_of(&action.model);
        assert_eq!((model.value.as_str(), model.pos.line, model.pos.col), ("a.b", 3, 3));
        assert_eq!(literal_of(&action.view_mode).value, "list,gantt");
        assert_eq!(action.id.as_deref(), Some("a"));
    }

    #[test]
    fn tells_a_write_that_is_no_literal_from_no_write() {
        for fields in [
            r#"<field name="res_model" eval="'a.b'"/><field name="view_mode" eval="'list'"/>"#,
            r#"<field name="res_model" eval="'a.b'">a.b</field><field name="view_mode" eval="'list'">list</field>"#,
            r#"<field name="res_model"/><field name="view_mode"></field>"#,
            r#"<field name="res_model">  </field><field name="view_mode"> </field>"#,
        ] {
            let actions = action_facts(&action_record(fields));
            assert!(matches!(actions[0].model, ActionValue::Opaque), "{fields}");
            assert!(matches!(actions[0].view_mode, ActionValue::Opaque), "{fields}");
        }
        let actions = action_facts(&action_record(""));
        assert!(matches!(actions[0].model, ActionValue::Unset));
        assert!(matches!(actions[0].view_mode, ActionValue::Unset));
    }

    #[test]
    fn reads_an_act_window_tag_at_its_attributes() {
        let text = "<odoo>\n<act_window id=\"a\" res_model=\"a.b\" view_mode=\"gantt\"/>\n</odoo>";
        let facts = analyze(text);
        let action = &facts.actions[0];
        let model = literal_of(&action.model);
        assert_eq!((model.value.as_str(), model.pos.line, model.pos.col), ("a.b", 2, 31));
        assert_eq!(literal_of(&action.view_mode).value, "gantt");
        assert_eq!(
            actions(r#"<act_window id="a" res_model=" " view_mode=""/>"#),
            [(None, None)]
        );
        let bare = action_facts(r#"<act_window id="a"/>"#);
        assert!(matches!(bare[0].view_mode, ActionValue::Opaque));
    }

    #[test]
    fn reads_the_views_an_action_names() {
        let named = |fields: &str| {
            let action = action_facts(&action_record(fields)).remove(0);
            (action.view_kinds, action.view_of_any_kind)
        };
        let none = (vec![], false);
        let kinds = |k: &[&str]| (k.iter().map(|k| (*k).to_owned()).collect::<Vec<_>>(), false);
        assert_eq!(named(r#"<field name="view_id" ref="v"/>"#), none);
        assert_eq!(
            named(r#"<field name="context">{'gantt_view_ref': 'm.v'}</field>"#),
            none
        );
        assert_eq!(
            named(
                r#"<field name="view_ids" eval="[(5, 0, 0), (0, 0, {'view_mode': 'gantt', 'view_id': ref('v')}), Command.create(dict(view_mode='map'))]"/>"#
            ),
            kinds(&["gantt"])
        );
        assert_eq!(
            named(r#"<field name="view_ids" eval="[(0, 0, {'view_mode': 'gantt'}), (4, ref('row'))]"/>"#),
            (vec![], true)
        );
        assert_eq!(
            named(r#"<field name="view_ids" eval="[(6, 0, [ref('row')])]"/>"#),
            (vec![], true)
        );
        assert_eq!(named(r#"<field name="view_ids" eval="[(5, 0, 0)]"/>"#), none);
        assert_eq!(named(r#"<field name="view_ids" eval=""/>"#), none);
        assert_eq!(named(r#"<field name="view_ids"/>"#), none);
    }

    #[test]
    fn reads_the_last_context_of_an_action() {
        let kinds_of = |fields: &str| action_facts(&action_record(fields)).remove(0).context_kinds;
        let some = |k: &[&str]| Some(k.iter().map(|k| (*k).to_owned()).collect::<Vec<_>>());
        assert_eq!(kinds_of(""), None);
        assert_eq!(kinds_of(r#"<field name="context"/>"#), some(&[]));
        assert_eq!(
            kinds_of(r#"<field name="context">{'gantt_view_ref': 'm.v', 'x': 1}</field>"#),
            some(&["gantt"])
        );
        assert_eq!(
            kinds_of(r#"<field name="context" eval="dict(map_view_ref='m.v')"/>"#),
            some(&["map"])
        );
        assert_eq!(
            kinds_of(r#"<field name="context" eval="">{'gantt_view_ref': 'm.v'}</field>"#),
            some(&["gantt"])
        );
        assert_eq!(
            kinds_of(r#"<field name="context">{'gantt_view_ref': 'm.v'}</field><field name="context">{}</field>"#),
            some(&[])
        );
    }

    #[test]
    fn reads_a_view_id_as_odoo_does() {
        let view_id = |field: &str| action_facts(&action_record(field)).remove(0).view_id;
        let by_ref = |id: &str| ActionViewId::Ref(id.to_owned());
        assert_eq!(view_id(""), ActionViewId::Unset);
        for field in [
            r#"<field name="view_id" ref="v"/>"#,
            r#"<field name="view_id" eval="ref('v')"/>"#,
            r#"<field name="view_id" ref="" eval="ref('v')"/>"#,
            r#"<field name="view_id" eval="False"/><field name="view_id" ref="v"/>"#,
        ] {
            assert_eq!(view_id(field), by_ref("v"), "{field}");
        }
        for field in [
            r#"<field name="view_id" search="[('name', '=', 'v')]"/>"#,
            r#"<field name="view_id" search="[('name', '=', 'v')]" eval="False"/>"#,
            r#"<field name="view_id" search="[('name', '=', 'v')]" ref="v"/>"#,
            r#"<field name="view_id">7</field>"#,
            r#"<field name="view_id" eval="x"/>"#,
            r#"<field name="view_id" eval="ref('v') or False"/>"#,
        ] {
            assert_eq!(view_id(field), ActionViewId::Unread, "{field}");
        }
        for field in [
            r#"<field name="view_id"/>"#,
            r#"<field name="view_id" ref=""/>"#,
            r#"<field name="view_id" search=""/>"#,
            r#"<field name="view_id" eval="False"/>"#,
            r#"<field name="view_id" ref="" eval="False"/>"#,
            r#"<field name="view_id" eval="0"/>"#,
            r#"<field name="view_id" ref="v"/><field name="view_id" eval="False"/>"#,
        ] {
            assert_eq!(view_id(field), ActionViewId::Cleared, "{field}");
        }
    }

    #[test]
    fn reads_the_last_model_and_mode_of_an_action() {
        let fields = r#"<field name="res_model">a.b</field><field name="res_model">c.d</field>
            <field name="view_mode">gantt</field><field name="view_mode">map</field>"#;
        assert_eq!(
            actions(&action_record(fields)),
            [(Some("c.d".to_owned()), Some("map".to_owned()))]
        );
        let empty_eval = r#"<field name="res_model" eval="">a.b</field><field name="view_mode" eval="">gantt</field>"#;
        assert_eq!(
            actions(&action_record(empty_eval)),
            [(Some("a.b".to_owned()), Some("gantt".to_owned()))]
        );
    }

    #[test]
    fn reads_the_views_an_act_window_tag_names() {
        let action = action_facts(r#"<act_window id="a" view_id="v" context="{'map_view_ref': 'm.v'}"/>"#).remove(0);
        assert_eq!(action.context_kinds, Some(vec!["map".to_owned()]));
        assert_eq!(action.view_id, ActionViewId::Ref("v".to_owned()));
        assert!(action.view_kinds.is_empty());
        assert!(!action.view_of_any_kind);
        for tag in [r#"<act_window id="a" view_id=""/>"#, r#"<act_window id="a"/>"#] {
            let action = action_facts(tag).remove(0);
            assert_eq!(action.view_id, ActionViewId::Cleared, "{tag}");
            assert_eq!(action.context_kinds, Some(vec![]), "{tag}");
        }
    }

    #[test]
    fn reads_a_view_row_as_a_write_to_its_action() {
        let row = |fields: &str| format!(r#"<record id="r" model="ir.actions.act_window.view">{fields}</record>"#);
        let rows = action_facts(&row(
            r#"<field name="view_mode">gantt</field><field name="view_id" ref="v"/><field name="act_window_id" ref="m.a"/>"#,
        ));
        assert_eq!(rows[0].id.as_deref(), Some("m.a"));
        assert_eq!(rows[0].view_kinds, ["gantt"]);
        assert!(!rows[0].view_of_any_kind);
        let hidden = action_facts(&row(
            r#"<field name="view_mode" eval="'gantt'"/><field name="view_id" ref="v"/><field name="act_window_id" ref="a"/>"#,
        ));
        assert!(hidden[0].view_of_any_kind);
        let last_mode = action_facts(&row(
            r#"<field name="view_mode">gantt</field><field name="view_mode">map</field><field name="view_id" ref="v"/><field name="act_window_id" ref="a"/>"#,
        ));
        assert_eq!(last_mode[0].view_kinds, ["map"]);
        let cleared = row(
            r#"<field name="view_mode">gantt</field><field name="view_id" ref="v"/><field name="view_id" eval="False"/><field name="act_window_id" ref="a"/>"#,
        );
        assert!(action_facts(&cleared).is_empty());
        let no_view = row(r#"<field name="view_mode">gantt</field><field name="act_window_id" ref="a"/>"#);
        assert!(action_facts(&no_view).is_empty());
        let no_action = row(r#"<field name="view_mode">gantt</field><field name="view_id" ref="v"/>"#);
        assert!(action_facts(&no_action).is_empty());
        let searched = action_facts(&row(
            r#"<field name="view_mode">gantt</field><field name="view_id" search="[]"/><field name="act_window_id" ref="a"/>"#,
        ));
        assert_eq!(searched[0].view_kinds, ["gantt"]);
    }

    #[test]
    fn reads_the_action_of_a_row_as_odoo_does() {
        let action_of = |fields: &str| {
            let head = r#"<field name="view_mode">gantt</field><field name="view_id" ref="v"/>"#;
            let row = format!(r#"<record id="r" model="ir.actions.act_window.view">{head}{fields}</record>"#);
            action_facts(&row).pop().map(|a| a.id.unwrap())
        };
        for (fields, action) in [
            (r#"<field name="act_window_id" eval="ref('m.a')"/>"#, "m.a"),
            (r#"<field name="act_window_id" eval="ref('a', False)"/>"#, "a"),
            (r#"<field name="act_window_id" ref="" eval="ref('a')"/>"#, "a"),
            (r#"<field name="act_window_id" ref="a" eval="ref('b')"/>"#, "a"),
            (
                r#"<field name="act_window_id" ref="b"/><field name="act_window_id" eval="ref('a')"/>"#,
                "a",
            ),
        ] {
            assert_eq!(action_of(fields).as_deref(), Some(action), "{fields}");
        }
        for fields in [
            r#"<field name="act_window_id" search="[]" ref="a"/>"#,
            r#"<field name="act_window_id" eval="False"/>"#,
            r#"<field name="act_window_id" eval="ref('a') or False"/>"#,
            r#"<field name="act_window_id" ref="a"/><field name="act_window_id" eval="False"/>"#,
        ] {
            assert_eq!(action_of(fields), None, "{fields}");
        }
    }

    #[test]
    fn reads_a_row_nested_in_view_ids_for_the_enclosing_action() {
        let nested = |row: &str| {
            let record = format!(r#"<record model="ir.actions.act_window.view">{row}</record>"#);
            action_facts(&action_record(&format!(r#"<field name="view_ids">{record}</field>"#)))
        };
        let actions = nested(r#"<field name="view_mode">gantt</field><field name="view_id" ref="v"/>"#);
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].id.as_deref(), Some("a"));
        assert_eq!(actions[0].nested_view_kinds, ["gantt"]);
        assert!(!actions[0].nested_view_of_any_kind);
        assert!(actions[0].view_kinds.is_empty());
        assert!(actions[0].row_names_view("gantt", true));
        assert!(!actions[0].row_names_view("gantt", false));

        let searched = nested(r#"<field name="view_mode">gantt</field><field name="view_id" search="[]"/>"#);
        assert_eq!(searched[0].nested_view_kinds, ["gantt"]);

        let hidden = nested(r#"<field name="view_mode" eval="'gantt'"/><field name="view_id" ref="v"/>"#);
        assert!(hidden[0].nested_view_of_any_kind);
        assert!(!hidden[0].view_of_any_kind);
        assert!(hidden[0].row_names_view("gantt", true));
        assert!(!hidden[0].row_names_view("gantt", false));

        let no_view = nested(r#"<field name="view_mode">gantt</field>"#);
        assert!(no_view[0].nested_view_kinds.is_empty());
        assert!(!no_view[0].nested_view_of_any_kind);

        let elsewhere = nested(
            r#"<field name="view_mode">gantt</field><field name="view_id" ref="v"/><field name="act_window_id" ref="b"/>"#,
        );
        assert_eq!(elsewhere.len(), 1);
        assert_eq!(elsewhere[0].id.as_deref(), Some("a"));
        assert_eq!(elsewhere[0].nested_view_kinds, ["gantt"]);
    }

    fn rule_of(fields: &str) -> RuleFact {
        let record = format!(r#"<odoo><record id="r" model="ir.rule">{fields}</record></odoo>"#);
        analyze(&record).rules.remove(0)
    }

    fn model_of(field: &str) -> (Option<String>, bool) {
        let rule = rule_of(field);
        let value = match rule.model {
            ActionValue::Literal(m) => Some(m.value),
            ActionValue::Opaque => None,
            ActionValue::Unset => panic!("{field}"),
        };
        (value, rule.by_name)
    }

    #[test]
    fn reads_the_forms_of_a_rule_model() {
        let id = |id: &str| (Some(id.to_owned()), false);
        assert_eq!(model_of(r#"<field name="model_id" ref="model_a"/>"#), id("model_a"));
        assert_eq!(model_of(r#"<field name="model_id" ref="m.model_a"/>"#), id("m.model_a"));
        assert_eq!(
            model_of(r#"<field name="model_id" eval="ref('m.model_a')"/>"#),
            id("m.model_a")
        );
        assert_eq!(
            model_of(r#"<field name="model_id" search="[('model', '=', 'a.b')]"/>"#),
            (Some("a.b".to_owned()), true)
        );
        assert_eq!(
            model_of(r#"<field name="model_id" model="ir.model" search="[('model', '=', 'a.b')]"/>"#),
            (Some("a.b".to_owned()), true)
        );
    }

    #[test]
    fn a_rule_model_only_a_run_tells_is_no_literal() {
        for field in [
            r#"<field name="model_id" eval="ref('a') or ref('b')"/>"#,
            r#"<field name="model_id" eval="False"/>"#,
            r#"<field name="model_id" search="[('name', '=', 'a')]"/>"#,
            r#"<field name="model_id" search="[('model', '=', 'a')]" use="model"/>"#,
            r#"<field name="model_id" search="[('model', '=', 'a')]" model="ir.ui.view"/>"#,
            r#"<field name="model_id" ref=""/>"#,
            r#"<field name="model_id"/>"#,
            r#"<field name="model_id">3</field>"#,
        ] {
            assert_eq!(model_of(field), (None, false), "{field}");
        }
    }

    #[test]
    fn a_nested_rule_takes_the_model_of_its_parent() {
        let text = r#"<odoo><record id="m" model="ir.model"><field name="rule_ids">
            <record id="r" model="ir.rule"><field name="model_id" ref="model_a"/></record>
        </field></record></odoo>"#;
        assert!(matches!(analyze(text).rules[0].model, ActionValue::Opaque));
    }

    #[test]
    fn reads_the_domain_of_a_rule() {
        let rule = rule_of("<field name=\"domain_force\">\n  [('a', '=', 1)]\n</field>");
        assert_eq!(rule.fields[0].value, "a");
        assert_eq!(rule.fields[0].pos.line, 2);
        let rule = rule_of(r#"<field name="domain_force" eval="[('b', '=', 1)]"/>"#);
        assert_eq!(rule.fields[0].value, "b");
        let access = analyze(
            r#"<odoo><record id="r" model="ir.access"><field name="domain">[('c', '=', 1)]</field></record></odoo>"#,
        );
        assert_eq!(access.rules[0].fields[0].value, "c");
    }

    #[test]
    fn a_rule_domain_only_a_run_tells_replaces_without_naming_fields() {
        for field in [
            r#"<field name="domain_force" ref="a"/>"#,
            r#"<field name="domain_force" search="[]"/>"#,
            r#"<field name="domain_force" file="a.txt"/>"#,
            r#"<field name="domain_force" type="xml"><a/></field>"#,
            r#"<field name="domain_force" type="char">[('a', '=', 1)]</field>"#,
            r#"<field name="domain_force"><function model="m" name="n"/></field>"#,
            r#"<field name="domain_force" type="xml"></field>"#,
            "<field name=\"domain_force\">\n  </field>",
        ] {
            let rule = rule_of(field);
            assert!(matches!(rule.domain, ActionValue::Opaque), "{field}");
            assert!(rule.fields.is_empty(), "{field}");
        }
        let unset = rule_of(r#"<field name="name">a</field>"#);
        assert!(matches!(unset.domain, ActionValue::Unset));
    }

    #[test]
    fn a_false_or_empty_rule_domain_is_an_empty_literal() {
        let empty = |field: &str| matches!(&rule_of(field).domain, ActionValue::Literal(d) if d.value.is_empty());
        for field in [
            r#"<field name="domain_force"/>"#,
            r#"<field name="domain_force"></field>"#,
            r#"<field name="domain_force" eval=""/>"#,
            r#"<field name="domain_force" eval=" False "/>"#,
            r#"<field name="domain_force" eval="None"/>"#,
            r#"<field name="domain_force" eval="''"/>"#,
        ] {
            assert!(empty(field), "{field}");
            assert!(rule_of(field).fields.is_empty(), "{field}");
        }
        for field in [
            r#"<field name="domain_force">False</field>"#,
            r#"<field name="domain_force" eval="0"/>"#,
            "<field name=\"domain_force\">\n  </field>",
            r#"<field name="domain_force" type="xml"/>"#,
        ] {
            assert!(!empty(field), "{field}");
        }
    }

    #[test]
    fn reads_the_groups_and_permissions_of_a_rule() {
        let rule = rule_of(
            r#"<field name="groups" eval="[(4, ref('base.group_portal'))]"/><field name="perm_read" eval="False"/><field name="perm_write">0</field><field name="perm_create" eval="x"/>"#,
        );
        assert_eq!(rule.groups.added, ["base.group_portal"]);
        assert!(rule.groups.keeps && !rule.groups.opaque && !rule.access);
        let set = Flag::Set;
        assert_eq!(rule.perms, [set(false), set(false), Flag::Opaque, Flag::Unset]);
        let unset = rule_of(r#"<field name="name">a</field>"#);
        assert!(unset.groups.keeps && unset.groups.added.is_empty() && !unset.groups.opaque);
        assert_eq!(unset.perms, [Flag::Unset; 4]);
        for field in [
            r#"<field name="groups" eval="[(3, ref('base.group_portal'))]"/>"#,
            r#"<field name="groups" search="[]"/>"#,
            r#"<field name="groups" eval="[(4, ref('a'))]" ref="b"/>"#,
        ] {
            assert!(rule_of(field).groups.opaque, "{field}");
        }
        let replaced = rule_of(r#"<field name="groups" eval="[Command.set([ref('a')])]"/>"#).groups;
        assert_eq!(replaced.added, ["a"]);
        assert!(!replaced.keeps);
    }

    #[test]
    fn reads_the_group_and_operation_of_an_access_record() {
        let access = |fields: &str| {
            let record = format!(r#"<odoo><record id="r" model="ir.access">{fields}</record></odoo>"#);
            analyze(&record).rules.remove(0)
        };
        let read = access(r#"<field name="group_id" ref="base.group_portal"/><field name="operation">r</field>"#);
        assert!(read.access);
        assert_eq!(read.groups.added, ["base.group_portal"]);
        assert!(!read.groups.keeps && !read.groups.opaque);
        let set = Flag::Set;
        assert_eq!(read.perms, [set(true), set(false), set(false), set(false)]);
        let by_eval = access(r#"<field name="group_id" eval="ref('g')"/><field name="operation" eval="'cud'"/>"#);
        assert_eq!(by_eval.groups.added, ["g"]);
        assert_eq!(by_eval.perms, [set(false), set(true), set(true), set(true)]);
        let cleared = access(r#"<field name="group_id" eval="False"/><field name="operation" eval="op"/>"#);
        assert!(cleared.groups.added.is_empty() && !cleared.groups.opaque);
        assert_eq!(cleared.perms, [Flag::Opaque; 4]);
        assert!(access(r#"<field name="group_id" eval="group"/>"#).groups.opaque);
    }

    #[test]
    fn reads_a_flag_as_the_loader_does() {
        let perm = |field: &str| rule_of(&format!(r#"<field name="perm_read" {field}"#)).perms[0];
        assert_eq!(perm(r#"eval="True"/>"#), Flag::Set(true));
        assert_eq!(perm(r#"eval=" 1 "/>"#), Flag::Set(true));
        assert_eq!(perm(r#"eval="None"/>"#), Flag::Set(false));
        assert_eq!(perm(r#"eval="0"/>"#), Flag::Set(false));
        assert_eq!(perm(r#"eval="2"/>"#), Flag::Opaque);
        assert_eq!(perm(r#"eval="not x"/>"#), Flag::Opaque);
        assert_eq!(perm(r#"ref="a"/>"#), Flag::Opaque);
        assert_eq!(perm(">off</field>"), Flag::Set(false));
        assert_eq!(perm("> False </field>"), Flag::Set(true));
        assert_eq!(perm("/>"), Flag::Set(true));
    }

    #[test]
    fn reads_a_string_eval_as_the_loader_converts_it() {
        let perm = |eval: &str, series: &str| {
            let record = format!(
                r#"<odoo><record id="r" model="ir.rule"><field name="perm_read" eval="{eval}"/></record></odoo>"#
            );
            super::analyze(&record, Series::parse(series)).rules.remove(0).perms[0]
        };
        for series in ["10.0", "18.0", "none"] {
            for eval in ["''", "'yes'", "' 0'", "'False '"] {
                assert_eq!(perm(eval, series), Flag::Set(true), "{eval} {series}");
            }
            for eval in ["'0'", "'False'", "'OFF'", "&quot;false&quot;"] {
                assert_eq!(perm(eval, series), Flag::Set(false), "{eval} {series}");
            }
        }
        for eval in ["'0'", "'False'", "'off'", "' '"] {
            assert_eq!(perm(eval, "9.0"), Flag::Set(true), "{eval}");
        }
        assert_eq!(perm("''", "9.0"), Flag::Set(false));
        assert_eq!(perm("False", "9.0"), Flag::Set(false));
        assert_eq!(perm("x", "9.0"), Flag::Opaque);
    }

    #[test]
    fn reads_text_before_10_0_as_the_string_stored() {
        let rule = |fields: &str| {
            let record = format!(r#"<odoo><record id="r" model="ir.rule">{fields}</record></odoo>"#);
            super::analyze(&record, Series::parse("9.0")).rules.remove(0)
        };
        for text in ["0", "false", "OFF", " "] {
            let fields = format!(r#"<field name="perm_read">{text}</field><field name="active">{text}</field>"#);
            let read = rule(&fields);
            assert_eq!(read.perms[0], Flag::Set(true), "{text:?}");
            assert_eq!(read.active, Some(true), "{text:?}");
        }
        let empty = rule(r#"<field name="perm_read"/><field name="active"></field>"#);
        assert_eq!(empty.perms[0], Flag::Set(false));
        assert_eq!(empty.active, Some(false));
    }

    #[test]
    fn reads_empty_text_as_none_on_10_0_and_as_a_string_from_11_0() {
        let rule = |series: &str| {
            let fields =
                r#"<field name="perm_read"/><field name="active"></field><field name="perm_write" eval="''"/>"#;
            let record = format!(r#"<odoo><record id="r" model="ir.rule">{fields}</record></odoo>"#);
            super::analyze(&record, Series::parse(series)).rules.remove(0)
        };
        let ten = rule("10.0");
        assert_eq!((ten.perms[0], ten.active), (Flag::Set(false), Some(false)));
        assert_eq!(ten.perms[1], Flag::Set(true));
        for series in ["11.0", "18.0", "none"] {
            let read = rule(series);
            assert_eq!((read.perms[0], read.active), (Flag::Set(true), Some(true)), "{series}");
            assert_eq!(read.perms[1], Flag::Set(true), "{series}");
        }
    }

    #[test]
    fn reads_active_as_the_loader_does() {
        let active = |field: &str| rule_of(field).active;
        assert_eq!(active(r#"<field name="name">a</field>"#), None);
        for field in [
            r#"<field name="active" eval="True"/>"#,
            r#"<field name="active" eval="1"/>"#,
            r#"<field name="active">True</field>"#,
            r#"<field name="active">yes</field>"#,
            r#"<field name="active"> False </field>"#,
            r#"<field name="active"/>"#,
        ] {
            assert_eq!(active(field), Some(true), "{field}");
        }
        for field in [
            r#"<field name="active" eval="False"/>"#,
            r#"<field name="active" eval="0"/>"#,
            r#"<field name="active" eval="not x"/>"#,
            r#"<field name="active">False</field>"#,
            r#"<field name="active">0</field>"#,
            r#"<field name="active">OFF</field>"#,
            r#"<field name="active" ref="a"/>"#,
        ] {
            assert_eq!(active(field), Some(false), "{field}");
        }
    }

    #[test]
    fn an_access_record_grants_with_a_group_only() {
        let access = |model: &str, group: &str| {
            let record = format!(
                r#"<odoo><record id="a" model="{model}"><field name="model_id" ref="model_a"/>{group}</record></odoo>"#
            );
            analyze(&record).access.len()
        };
        assert_eq!(access("ir.model.access", ""), 1);
        assert_eq!(access("ir.access", ""), 0);
        assert_eq!(access("ir.access", r#"<field name="group_id" ref=""/>"#), 0);
        assert_eq!(access("ir.access", r#"<field name="group_id" ref="g"/>"#), 1);
        assert_eq!(access("ir.access", r#"<field name="group_id" eval="ref('g')"/>"#), 1);
    }

    fn server_facts(model: &str, fields: &str) -> DataFacts {
        analyze(&format!(
            "<odoo>\n<record id=\"s\" model=\"{model}\">\n{fields}</record>\n</odoo>"
        ))
    }

    fn server(fields: &str) -> ServerFact {
        server_facts("ir.actions.server", fields).servers.remove(0)
    }

    fn at(call: &Spanned<String>) -> (&str, u32, u32) {
        (call.value.as_str(), call.pos.line, call.pos.col)
    }

    #[test]
    fn reads_a_record_of_both_models() {
        let fields = r#"<field name="model_id" ref="model_a"/><field name="code">model.a()</field>"#;
        let server = server_facts("ir.actions.server", fields).servers.remove(0);
        assert!(!server.cron && server.writes_code);
        assert_eq!(server.id.as_deref(), Some("s"));
        assert_eq!(literal_of(&server.model).value, "model_a");
        assert!(server_facts("ir.cron", fields).servers[0].cron);
        assert!(server_facts("base.automation", fields).servers.is_empty());
    }

    #[test]
    fn a_call_sits_on_its_line_past_decoded_newlines() {
        let code = |text: &str| server(&format!(r#"<field name="code">{text}</field>"#)).calls;
        assert_eq!(at(&code("model.a()")[0]), ("a", 3, 26));
        assert_eq!(at(&code("\n\n  \nmodel.a()\n")[0]), ("a", 6, 7));
        assert_eq!(at(&code("x = 1\nif x:\n    model.a()\n")[0]), ("a", 5, 11));
    }

    #[test]
    fn cdata_and_entities_keep_the_line() {
        let code = |text: &str| server(&format!(r#"<field name="code">{text}</field>"#)).calls;
        assert_eq!(at(&code("<![CDATA[model.a()]]>")[0]), ("a", 3, 35));
        assert_eq!(at(&code("<![CDATA[\n\n  model.a()\n]]>")[0]), ("a", 5, 9));
        assert_eq!(at(&code("\n<![CDATA[\nmodel.a()]]>")[0]), ("a", 5, 7));
        assert_eq!(at(&code("x = 1 &lt; 2\nmodel.a()")[0]), ("a", 4, 7));
        assert_eq!(at(&code("&#32;\nmodel.a()")[0]), ("a", 4, 7));
        assert_eq!(at(&code("a = 1&#10;model.a()")[0]), ("a", 3, 36));
        assert_eq!(at(&code("a = 1 &lt; 2; model.a()")[0]), ("a", 3, 40));
        assert_eq!(at(&code("<![CDATA[a = 1]]>; model.a()")[0]), ("a", 3, 45));
        assert_eq!(at(&code("<![CDATA[a = 1]]><![CDATA[; model.a()]]>")[0]), ("a", 3, 54));
        assert_eq!(at(&code("<![CDATA[]]>model.a()")[0]), ("a", 3, 38));
        assert_eq!(at(&code("s = \"&#xE9;\"; model.a()")[0]), ("a", 3, 40));
        assert_eq!(at(&code("<![CDATA[a = \"&lt;\"; model.a()]]>")[0]), ("a", 3, 47));
        assert_eq!(at(&code("a = 1\r\nmodel.a()")[0]), ("a", 4, 7));
        assert_eq!(at(&code("<![CDATA[a = 1\r\nmodel.a()]]>")[0]), ("a", 4, 7));
        let name = report_facts("<field name=\"report_name\">&#32;a.b</field>\n");
        assert_eq!(at(literal_of(&name.template)), ("a.b", 3, 32));
    }

    #[test]
    fn a_doctype_entity_ends_the_count() {
        let code = |text: &str| {
            let doc = format!(
                "<!DOCTYPE odoo [<!ENTITY e \"a\">]>\n<odoo>\n<record id=\"s\" model=\"ir.cron\">\n\
                 <field name=\"code\">{text}</field></record>\n</odoo>"
            );
            analyze(&doc).servers.remove(0).calls
        };
        assert_eq!(at(&code("b = &e;; model.a()")[0]), ("a", 4, 24));
        assert_eq!(code("&e; = 1; model.a()")[0].value, "a");
    }

    #[test]
    fn source_offset_stops_where_it_cannot_count() {
        assert_eq!(source_offset("&lt", 0, "<x", 1), 0);
        assert_eq!(source_offset("ab", 0, "abc", 3), 2);
        assert_eq!(source_offset("\u{e9}&lt;x", 0, "a\u{e9}<x", 4), 2);
    }

    #[test]
    fn only_the_first_line_of_the_text_is_stripped() {
        let refused = |text: &str| {
            let fact = server_facts("ir.cron", &format!(r#"<field name="code">{text}</field>"#));
            fact.servers[0].refused.as_ref().map(|r| r.pos.line)
        };
        assert_eq!(refused("\n    a()\n    b()\n"), Some(5));
        assert_eq!(refused("\n    a()\nb()\n"), None);
    }

    #[test]
    fn reads_a_syntax_error_in_any_state() {
        let fact = server(r#"<field name="state">object_write</field><field name="code">return</field>"#);
        let refused = fact.refused.unwrap();
        assert_eq!((refused.pos.line, refused.pos.col), (3, 60));
        assert_eq!(refused.value, "'return' outside function");
    }

    #[test]
    fn keeps_the_last_field_of_a_name() {
        let fact = server(
            r#"<field name="model_id" ref="a"/><field name="model_id" ref="b"/>
<field name="state">other</field><field name="state">code</field>
<field name="code">model.a()</field><field name="code">model.b()</field>"#,
        );
        assert_eq!(literal_of(&fact.model).value, "b");
        assert_eq!(fact.state, Some(true));
        assert_eq!(fact.calls.iter().map(|c| c.value.as_str()).collect::<Vec<_>>(), ["b"]);
        let cleared = server(r#"<field name="code">model.a()</field><field name="code"/>"#);
        assert!(cleared.writes_code && cleared.calls.is_empty());
    }

    #[test]
    fn a_value_from_elsewhere_than_the_text_reads_no_code() {
        for field in [
            r#"<field name="code" eval="'a'">(</field>"#,
            r#"<field name="code" ref="a">(</field>"#,
            r#"<field name="code" search="[]">(</field>"#,
            r#"<field name="code" file="a.py">(</field>"#,
            r#"<field name="code" type="char">(</field>"#,
            r#"<field name="code" type="">(</field>"#,
            r#"<field name="code"><function model="m" name="n"><value>(</value></function></field>"#,
            r#"<field name="code"><!-- a -->(</field>"#,
            r#"<field name="code"><?a b?>(</field>"#,
            r#"<field name="code">a()<!-- a -->(</field>"#,
            r#"<field name="code"> </field>"#,
            r#"<field name="code"/>"#,
        ] {
            let facts = server_facts("ir.cron", field);
            assert!(facts.servers[0].refused.is_none(), "{field}");
            assert!(facts.servers[0].writes_code, "{field}");
        }
    }

    #[test]
    fn an_empty_attribute_leaves_the_text_to_read() {
        let fact = server(r#"<field name="code" eval="" ref="" search="" file="">model.a()</field>"#);
        assert_eq!(fact.calls[0].value, "a");
    }

    #[test]
    fn tells_a_state_that_runs_code() {
        let state = |fields: &str| server(fields).state;
        assert_eq!(state(r#"<field name="name">a</field>"#), None);
        assert_eq!(state(r#"<field name="state">code</field>"#), Some(true));
        for field in [
            r#"<field name="state">multi</field>"#,
            r#"<field name="state"> code </field>"#,
            r#"<field name="state"/>"#,
            r#"<field name="state" eval="'code'"/>"#,
            r#"<field name="state" eval="'code'">code</field>"#,
            r#"<field name="state" type="char">code</field>"#,
        ] {
            assert_eq!(state(field), Some(false), "{field}");
        }
    }

    #[test]
    fn reads_a_nested_server_action() {
        let facts = analyze(
            r#"<odoo><record id="p" model="ir.actions.server"><field name="child_ids">
            <record id="c" model="ir.actions.server"><field name="model_id" ref="m"/></record>
        </field></record></odoo>"#,
        );
        assert_eq!(facts.servers.len(), 2);
        assert_eq!(literal_of(&facts.servers[0].model).value, "m");
        assert_eq!(literal_of(&facts.servers[0].parent).value, "p");
        assert!(matches!(facts.servers[1].parent, ActionValue::Unset));
    }

    #[test]
    fn reads_the_parent_of_a_server_action() {
        let parent = |fields: &str| server(fields).parent;
        assert!(matches!(parent(r#"<field name="name">a</field>"#), ActionValue::Unset));
        for field in [
            r#"<field name="parent_id" ref="p"/>"#,
            r#"<field name="parent_id" eval="ref('p')"/>"#,
            r#"<field name="parent_id" ref="q"/><field name="parent_id" ref="p"/>"#,
        ] {
            assert_eq!(literal_of(&parent(field)).value, "p", "{field}");
        }
        for field in [
            r#"<field name="parent_id" eval="False"/>"#,
            r#"<field name="parent_id" search="[]"/>"#,
            r#"<field name="parent_id">3</field>"#,
            r#"<field name="parent_id"/>"#,
            r#"<field name="parent_id" ref="p"/><field name="parent_id" eval="False"/>"#,
        ] {
            assert!(matches!(parent(field), ActionValue::Opaque), "{field}");
        }
    }

    #[test]
    fn a_nested_server_action_takes_the_enclosing_record_as_parent() {
        let nested = |outer: &str, field: &str| {
            let fields = r#"<field name="parent_id" ref="q"/>"#;
            let text = format!(
                r#"<odoo><record {outer}><field name="{field}">
                <record id="c" model="ir.actions.server">{fields}</record>
            </field></record></odoo>"#
            );
            analyze(&text).servers.remove(0).parent
        };
        let server = r#"id="p" model="ir.actions.server""#;
        assert_eq!(literal_of(&nested(server, "child_ids")).value, "p");
        assert_eq!(literal_of(&nested(server, "other_ids")).value, "q");
        for outer in [r#"model="ir.actions.server""#, r#"id="" model="ir.actions.server""#] {
            assert!(matches!(nested(outer, "child_ids"), ActionValue::Opaque), "{outer}");
        }
        for outer in [r#"id="p" model="ir.cron""#, r#"id="p" model="base.automation""#] {
            assert!(matches!(nested(outer, "child_ids"), ActionValue::Opaque), "{outer}");
        }
    }

    fn report_facts(fields: &str) -> ReportFact {
        let facts = analyze(&format!(
            "<odoo>\n<record id=\"r\" model=\"ir.actions.report\">\n{fields}</record>\n</odoo>"
        ));
        facts.reports.into_iter().next().unwrap()
    }

    fn report_tag(attrs: &str) -> ReportFact {
        let facts = analyze(&format!("<odoo>\n<report id=\"r\" {attrs}/>\n</odoo>"));
        facts.reports.into_iter().next().unwrap()
    }

    #[test]
    fn reads_a_report_record_at_its_text() {
        let fact = report_facts(
            r#"<field name="report_name"> a.b </field><field name="report_type">qweb-text</field>
<field name="print_report_name">x = 1</field>"#,
        );
        assert!(!fact.tag && fact.writes_file_name);
        assert_eq!(fact.id.as_deref(), Some("r"));
        assert_eq!(at(literal_of(&fact.template)), ("a.b", 3, 28));
        assert_eq!(fact.qweb, Some(true));
        assert_eq!(fact.refused.unwrap().pos.line, 4);
    }

    #[test]
    fn a_report_keeps_the_last_field_of_a_name() {
        let fact = report_facts(
            r#"<field name="report_name">a.b</field><field name="report_name">c.d</field>
<field name="report_type">qweb-pdf</field><field name="report_type">controller</field>
<field name="print_report_name">(</field><field name="print_report_name">a</field>"#,
        );
        assert_eq!(literal_of(&fact.template).value, "c.d");
        assert_eq!(fact.qweb, Some(false));
        assert!(fact.refused.is_none());
    }

    #[test]
    fn a_report_name_keeps_its_place_under_cdata() {
        let name = report_facts("<field name=\"report_name\"><![CDATA[\n  a.b\n]]></field>\n");
        assert_eq!(at(literal_of(&name.template)), ("a.b", 4, 3));
    }

    #[test]
    fn a_report_name_a_scan_cannot_look_up_holds_no_literal() {
        for field in [
            r#"<field name="report_name"/>"#,
            r#"<field name="report_name">  </field>"#,
            r#"<field name="report_name">42</field>"#,
            r#"<field name="report_name">a/b.xml</field>"#,
            r#"<field name="report_name">{{ a }}</field>"#,
            r#"<field name="report_name" eval="'a.b'">c.d</field>"#,
            r#"<field name="report_name" ref="a.b">c.d</field>"#,
            r#"<field name="report_name" search="[]">c.d</field>"#,
            r#"<field name="report_name" file="a.xml">c.d</field>"#,
            r#"<field name="report_name" type="char">c.d</field>"#,
            r#"<field name="report_name"><!-- a -->c.d</field>"#,
        ] {
            assert!(matches!(report_facts(field).template, ActionValue::Opaque), "{field}");
        }
    }

    #[test]
    fn tells_a_report_type_that_renders_qweb() {
        let qweb = |fields: &str| report_facts(fields).qweb;
        assert_eq!(qweb(r#"<field name="name">a</field>"#), None);
        for value in ["qweb-pdf", "qweb-html", "qweb-text"] {
            assert_eq!(
                qweb(&format!(r#"<field name="report_type">{value}</field>"#)),
                Some(true)
            );
        }
        for field in [
            r#"<field name="report_type">controller</field>"#,
            r#"<field name="report_type"> qweb-pdf </field>"#,
            r#"<field name="report_type"/>"#,
            r#"<field name="report_type" eval="'qweb-pdf'"/>"#,
            r#"<field name="report_type" eval="'py3o'">qweb-pdf</field>"#,
            r#"<field name="report_type" type="char">qweb-pdf</field>"#,
        ] {
            assert_eq!(qweb(field), Some(false), "{field}");
        }
    }

    #[test]
    fn a_file_name_refused_sits_past_cdata_and_entities() {
        let refused = |text: &str| {
            let fact = report_facts(&format!(r#"<field name="print_report_name">{text}</field>"#));
            let refused = fact.refused.unwrap();
            (refused.pos.line, refused.pos.col)
        };
        assert_eq!(refused("x = 1"), (3, 35));
        assert_eq!(refused("\n\n   x = 1\n"), (5, 6));
        assert_eq!(refused("<![CDATA[x = 1]]>"), (3, 44));
        assert_eq!(refused("<![CDATA[\n  x = 1\n]]>"), (4, 5));
        assert_eq!(refused("a &lt;\nb"), (3, 39));
    }

    #[test]
    fn a_blank_file_name_is_refused_and_an_empty_one_unset() {
        let refused = |field: &str| report_facts(field).refused.is_some();
        assert!(refused(r#"<field name="print_report_name">   </field>"#));
        assert!(refused("<field name=\"print_report_name\">\n</field>"));
        for field in [
            r#"<field name="print_report_name"/>"#,
            r#"<field name="print_report_name"></field>"#,
            r#"<field name="print_report_name"><![CDATA[]]></field>"#,
        ] {
            assert!(!refused(field), "{field}");
            assert!(report_facts(field).writes_file_name, "{field}");
        }
    }

    #[test]
    fn a_file_name_from_elsewhere_than_the_text_counts_as_a_write_only() {
        for field in [
            r#"<field name="print_report_name" eval="'a'">(</field>"#,
            r#"<field name="print_report_name" ref="a">(</field>"#,
            r#"<field name="print_report_name" search="[]">(</field>"#,
            r#"<field name="print_report_name" file="a.txt">(</field>"#,
            r#"<field name="print_report_name" type="char">(</field>"#,
            r#"<field name="print_report_name"><!-- a -->(</field>"#,
        ] {
            let fact = report_facts(field);
            assert!(fact.refused.is_none() && fact.writes_file_name, "{field}");
        }
        assert!(
            report_facts(r#"<field name="print_report_name" eval="">(</field>"#)
                .refused
                .is_some()
        );
    }

    #[test]
    fn reads_a_report_tag_at_its_attributes() {
        let fact = report_tag(r#"string="A" model="m" name="a.b" report_type="qweb-html" print_report_name="x = 1""#);
        assert!(fact.tag && fact.writes_file_name);
        assert_eq!(at(literal_of(&fact.template)), ("a.b", 2, 43));
        assert_eq!(fact.qweb, Some(true));
        assert_eq!(fact.refused.unwrap().pos.col, 93);
    }

    #[test]
    fn an_empty_name_of_a_report_tag_gives_no_target() {
        assert!(matches!(report_tag(r#"name="""#).template, ActionValue::Unset));
    }

    #[test]
    fn an_empty_optional_attribute_of_a_report_tag_writes_nothing() {
        let fact = report_tag(r#"report_type="" print_report_name="""#);
        assert_eq!(fact.qweb, None);
        assert!(!fact.writes_file_name);
        assert!(report_tag(r#"print_report_name="  ""#).refused.is_some());
        assert!(matches!(report_tag(r#"name="a/b.xml""#).template, ActionValue::Opaque));
        assert_eq!(report_tag(r#"report_type="controller""#).qweb, Some(false));
        assert_eq!(literal_of(&report_tag(r#"name=" a.b ""#).template).value, "a.b");
    }

    #[test]
    fn tells_a_view_kind_from_type_then_root() {
        let kind = |fields: &str| {
            let facts = analyze(&format!(
                r#"<odoo><record id="x" model="ir.ui.view">{fields}</record></odoo>"#
            ));
            facts.views[0].kind.clone()
        };
        let arch = |root: &str| format!(r#"<field name="arch" type="xml"><{root}/></field>"#);
        assert_eq!(kind(&arch("form")).as_deref(), Some("form"));
        assert_eq!(kind(&arch("t")), None);
        assert_eq!(kind(&arch("data")), None);
        assert_eq!(kind(&arch("xpath")), None);
        let typed = format!(r#"{}<field name="type">qweb</field>"#, arch("div"));
        assert_eq!(kind(&typed).as_deref(), Some(QWEB));
    }

    #[test]
    fn tells_a_primary_record_from_an_extension() {
        let primary = |fields: &str| view(&format!(r#"<record id="x" model="ir.ui.view">{fields}</record>"#)).primary;
        let parent = r#"<field name="inherit_id" ref="y"/>"#;
        let opaque = r#"<field name="inherit_id" eval="x"/>"#;
        let search = r#"<field name="inherit_id" search="[('name', '=', 'y')]"/>"#;
        let mode = r#"<field name="mode">primary</field>"#;
        for fields in [
            String::new(),
            r#"<field name="inherit_id" eval="False"/>"#.to_owned(),
            r#"<field name="inherit_id" ref=""/>"#.to_owned(),
            r#"<field name="inherit_id"/>"#.to_owned(),
            format!("{parent}{mode}"),
            format!("{mode}{parent}"),
            format!(r#"{parent}<field name="mode" eval="'primary'"/>"#),
            format!(r#"{parent}<field name="mode">extension</field>{mode}"#),
            format!(r#"{parent}<field name="inherit_id" eval="False"/>"#),
            format!("{search}{mode}"),
        ] {
            assert_eq!(primary(&fields), Some(true), "{fields}");
        }
        for fields in [
            parent,
            r#"<field name="inherit_id" eval="ref('y')"/>"#,
            r#"<field name="mode">extension</field>"#,
            r#"<field name="mode" eval="'extension'"/>"#,
        ] {
            assert_eq!(primary(fields), Some(false), "{fields}");
        }
        for fields in [
            search.to_owned(),
            opaque.to_owned(),
            r#"<field name="inherit_id" model="ir.model" search="[('model', '=', 'y')]"/>"#.to_owned(),
            r#"<field name="mode" eval="x"/>"#.to_owned(),
            format!(r#"{parent}<field name="mode" eval="x"/>"#),
        ] {
            assert_eq!(primary(&fields), None, "{fields}");
        }
    }

    #[test]
    fn tells_a_parent_the_view_does_not_read() {
        let unread =
            |parent: &str| view(&format!(r#"<record id="x" model="ir.ui.view">{parent}</record>"#)).unread_parent;
        assert!(!unread(""));
        assert!(!unread(r#"<field name="inherit_id" ref="y"/>"#));
        assert!(!unread(r#"<field name="inherit_id" eval="False"/>"#));
        assert!(!unread(
            r#"<field name="inherit_id" eval="ref('y')"/><field name="inherit_id" ref="y"/>"#
        ));
        assert!(unread(r#"<field name="inherit_id" eval="ref('y')"/>"#));
        assert!(unread(r#"<field name="inherit_id" search="[('name', '=', 'y')]"/>"#));
        assert!(unread(r#"<field name="inherit_id" ref="" eval="x"/>"#));
    }

    #[test]
    fn tells_a_primary_template_from_an_extension() {
        let primary = |attrs: &str| view(&format!(r#"<template id="x" {attrs}/>"#)).primary;
        assert_eq!(primary(""), Some(true));
        assert_eq!(primary(r#"inherit_id="""#), Some(true));
        assert_eq!(primary(r#"inherit_id="y" primary="True""#), Some(true));
        assert_eq!(primary(r#"inherit_id="y""#), Some(false));
        assert_eq!(primary(r#"inherit_id="y" primary="true""#), Some(false));
    }

    #[test]
    fn collects_the_key_of_views_and_pages() {
        let record = |model: &str| {
            let text =
                format!(r#"<odoo><record id="x" model="{model}"><field name="key">a.page</field></record></odoo>"#);
            analyze(&text).view_keys
        };
        assert_eq!(record("website.page"), ["a.page"]);
        assert_eq!(record("ir.ui.view"), ["a.page"]);
        assert_eq!(record("theme.ir.ui.view"), ["a.page"]);
        assert_eq!(record("website.controller.page"), ["a.page"]);
        assert!(record("res.partner").is_empty());
        let keyed = analyze(r#"<odoo><template id="x" key="a.b"/></odoo>"#);
        assert_eq!(keyed.view_keys, ["a.b"]);
    }

    #[test]
    fn names_a_template_by_its_id_else_its_t_name() {
        let names = |element: &str| {
            let facts = analyze(&format!("<odoo>{element}</odoo>"));
            assert!(facts.view_keys.is_empty());
            let defs: Vec<String> = facts.defs.into_iter().map(|d| d.id).collect();
            let views: Vec<Option<String>> = facts.views.into_iter().map(|v| v.id).collect();
            (defs, views)
        };
        let named = |id: &str| (vec![id.to_owned()], vec![Some(id.to_owned())]);
        assert_eq!(names(r#"<template t-name="a"/>"#), named("a"));
        assert_eq!(names(r#"<template id="a" t-name="b"/>"#), named("a"));
        assert_eq!(names(r#"<template id="" t-name="b"/>"#), named(""));
        assert_eq!(names("<template/>"), (vec![], vec![None]));
    }

    #[test]
    fn t_name_names_only_a_template() {
        let defs = |element: &str| analyze(&format!("<odoo>{element}</odoo>")).defs;
        assert!(defs(r#"<menuitem t-name="a"/>"#).is_empty());
        assert!(defs(r#"<record t-name="a" model="ir.ui.view"/>"#).is_empty());
    }

    #[test]
    fn only_a_record_and_a_template_read_forcecreate() {
        let no_create = |tag: &str| {
            let text = format!(r#"<odoo><{tag} id="a" model="m" forcecreate="False"/></odoo>"#);
            analyze(&text).defs[0].no_create
        };
        for tag in ["record", "template"] {
            assert!(no_create(tag), "{tag}");
        }
        for tag in ["menuitem", "act_window", "report", "asset"] {
            assert!(!no_create(tag), "{tag}");
        }
    }

    /// The codes of the markup a view record's arch drops, with `fields` before the arch and `arch`
    /// in it.
    fn arch_codes(fields: &str, arch: &str) -> Vec<&'static str> {
        let record = format!(
            r#"<record id="v" model="ir.ui.view">{fields}<field name="arch" type="xml">{arch}</field></record>"#
        );
        view(&record).dropped.into_iter().map(|d| d.code).collect()
    }

    #[test]
    fn reports_a_chatter_div_by_its_class_tokens() {
        assert_eq!(arch_codes("", r#"<form><div class="oe_chatter"/></form>"#), ["OD327"]);
        assert_eq!(
            arch_codes("", "<form><div class=\"o_x\toe_chatter \"/></form>"),
            ["OD327"]
        );
        for markup in [
            r#"<div class="oe_chatter_box"/>"#,
            r#"<div class="o_oe_chatter"/>"#,
            r#"<span class="oe_chatter"/>"#,
            "<chatter/>",
        ] {
            assert_eq!(arch_codes("", &format!("<form>{markup}</form>")), [""; 0], "{markup}");
        }
    }

    #[test]
    fn reports_a_kanban_box_template_without_a_card() {
        let kanban = |names: &str| {
            let templates: String = names
                .split(' ')
                .map(|n| format!(r#"<t t-name="{n}"><div/></t>"#))
                .collect();
            format!("<kanban><templates>{templates}</templates></kanban>")
        };
        assert_eq!(arch_codes("", &kanban("kanban-box")), ["OD326"]);
        assert_eq!(arch_codes("", &kanban("card")), [""; 0]);
        assert_eq!(arch_codes("", &kanban("kanban-box card")), [""; 0]);
        let form = format!(
            r#"<form>{}<field name="line_ids">{}</field></form>"#,
            kanban("card"),
            kanban("kanban-box")
        );
        assert_eq!(arch_codes("", &form), ["OD326"]);
    }

    #[test]
    fn a_spec_on_card_keeps_an_added_kanban_box_silent() {
        let parent = r#"<field name="inherit_id" ref="p"/>"#;
        let box_only = r#"<xpath expr="//templates" position="inside"><t t-name="kanban-box"/></xpath>"#;
        assert_eq!(arch_codes(parent, box_only), ["OD326"]);
        let beside = format!(r#"<data>{box_only}<t t-name="card" position="inside"><span/></t></data>"#);
        assert_eq!(arch_codes(parent, &beside), [""; 0]);
    }

    #[test]
    fn a_spec_reports_no_markup() {
        let parent = r#"<field name="inherit_id" ref="p"/>"#;
        let spec = r#"<div class="oe_chatter" position="replace"><chatter/></div>"#;
        assert_eq!(arch_codes(parent, spec), [""; 0]);
        let xpath = r#"<xpath expr="//div[hasclass('oe_chatter')]" position="replace"><chatter/></xpath>"#;
        assert_eq!(arch_codes(parent, xpath), [""; 0]);
        let added = r#"<xpath expr="//sheet" position="after"><div class="oe_chatter"/></xpath>"#;
        assert_eq!(arch_codes(parent, added), ["OD327"]);
    }

    fn escape_lines(view: &ViewFact) -> Vec<u32> {
        view.escapes.iter().map(|d| d.pos.line).collect()
    }

    #[test]
    fn records_the_escapes_an_arch_adds() {
        let template = view("<template id=\"t\">\n<div>\n<span t-esc=\"a\"/>\n<t t-out=\"b\"/></div></template>");
        assert_eq!(escape_lines(&template), [3]);
        let record = view(
            "<record id=\"v\" model=\"ir.ui.view\"><field name=\"arch\" type=\"xml\"><div>\n<span t-esc=\"a\"/></div></field></record>",
        );
        assert_eq!(escape_lines(&record), [2]);
    }

    #[test]
    fn a_spec_locates_an_escape_and_its_content_adds_one() {
        let spec = extension("<span t-esc=\"a\" position=\"replace\">\n<span t-esc=\"b\"/></span>");
        assert_eq!(escape_lines(&spec), [2]);
        let record = view(
            "<record id=\"v\" model=\"ir.ui.view\"><field name=\"inherit_id\" ref=\"p\"/><field name=\"arch\" type=\"xml\"><data><span t-esc=\"a\" position=\"replace\"/><xpath expr=\"//p\" position=\"after\">\n<em t-esc=\"b\"/></xpath></data></field></record>",
        );
        assert_eq!(escape_lines(&record), [2]);
    }

    /// The OD306 and OD325 findings of `facts`, wherever the facts hold them.
    fn renames(facts: &DataFacts) -> Vec<&RawDiag> {
        let views = facts.views.iter().flat_map(|v| v.renamed.iter().chain(&v.escapes));
        let all = facts.diags.iter().chain(views);
        all.filter(|d| matches!(d.code, "OD306" | "OD325")).collect()
    }

    /// `text` with the fixes of its OD306 and OD325 findings applied, and how many had none.
    fn fixed(text: &str) -> (String, usize) {
        let facts = analyze(text);
        let found = renames(&facts);
        assert!(!found.is_empty(), "{text}");
        let fixes: Vec<&Fix> = found.iter().filter_map(|d| d.fix.as_ref()).collect();
        let (out, outcomes) = crate::fix::apply(text.as_bytes(), &fixes);
        assert!(outcomes.iter().all(|o| *o == crate::fix::Outcome::Applied), "{text}");
        (String::from_utf8(out).unwrap(), found.len() - fixes.len())
    }

    /// Fixes every finding of `text`, then checks the result reads and reports none.
    fn rewrites(text: &str) -> String {
        let (out, left) = fixed(text);
        assert_eq!(left, 0, "{text}");
        let again = analyze(&out);
        assert!(!again.unreadable && renames(&again).is_empty(), "{out}");
        out
    }

    fn unfixed(text: &str) {
        assert_eq!(fixed(text), (text.to_owned(), 1), "{text}");
    }

    fn arch_record(arch: &str) -> String {
        format!(
            r#"<odoo><record id="v" model="ir.ui.view"><field name="arch" type="xml">{arch}</field></record></odoo>"#
        )
    }

    fn window_action(view_mode: &str) -> String {
        format!(
            r#"<odoo><record id="a" model="ir.actions.act_window"><field name="view_mode">{view_mode}</field></record></odoo>"#
        )
    }

    #[test]
    fn a_tree_becomes_a_list_in_both_its_tags() {
        let spaced = arch_record("<tree string=\"x\">\n<field name=\"a\"/></tree >");
        assert_eq!(
            rewrites(&spaced),
            arch_record("<list string=\"x\">\n<field name=\"a\"/></list >")
        );
        let empty = arch_record(r#"<form><field name="l"><tree/></field></form>"#);
        assert_eq!(rewrites(&empty), empty.replace("<tree/>", "<list/>"));
        let crlf = format!(
            "\u{feff}<?xml version=\"1.0\"?>\r\n{}\r\n",
            arch_record("<tree\r\n>\r\n</tree\r\n>")
        );
        assert_eq!(rewrites(&crlf), crlf.replace("tree", "list"));
    }

    #[test]
    fn a_prefixed_name_or_one_from_an_entity_stays() {
        unfixed(&arch_record(r#"<x:tree xmlns:x="urn:x"><field name="a"/></x:tree>"#));
        let entity = r#"<!DOCTYPE odoo [<!ENTITY t "<tree/>">]>"#;
        unfixed(&format!(
            "{entity}{}",
            arch_record(r#"<form><field name="l">&t;</field></form>"#)
        ));
        unfixed(r#"<odoo><template id="t"><span xmlns:x="urn:x" x:t-esc="a"/></template></odoo>"#);
        let entity = r#"<!DOCTYPE odoo [<!ENTITY s "<span t-esc='a'/>">]>"#;
        unfixed(&format!(
            "{entity}<odoo><template id=\"t\"><div>&s;</div></template></odoo>"
        ));
    }

    #[test]
    fn each_tree_of_a_view_mode_becomes_list() {
        assert_eq!(rewrites(&window_action(" tree , form")), window_action(" list , form"));
        assert_eq!(rewrites(&window_action("kanban,tree")), window_action("kanban,list"));
    }

    #[test]
    fn a_view_mode_listing_list_or_spelled_otherwise_stays() {
        for view_mode in [
            "tree,list",
            "tree, list",
            "tr&#101;e,form",
            "<![CDATA[tree,form]]>",
            "tree,form<!-- kanban -->",
            // The empty section leaves no node of its own, so only the end tag's place tells.
            "tree,form<![CDATA[]]>",
        ] {
            unfixed(&window_action(view_mode));
        }
        for indirect in [
            r#"eval="'list'""#,
            r#"ref="a""#,
            r#"search="[]""#,
            r#"file="a""#,
            r#"type="char""#,
        ] {
            let field = format!(r#"name="view_mode" {indirect}"#);
            unfixed(&window_action("tree,form").replace(r#"name="view_mode""#, &field));
        }
    }

    #[test]
    fn t_esc_becomes_t_out() {
        let template = "<odoo><template id=\"t\"><span class=\"a\" t-esc=\"a\" t-options=\"{}\"/></template></odoo>";
        assert_eq!(rewrites(template), template.replace("t-esc", "t-out"));
    }

    #[test]
    fn t_esc_beside_another_output_stays() {
        for other in ["t-out", "t-field", "t-raw"] {
            unfixed(&format!(
                "<odoo><template id=\"t\"><span t-esc=\"a\" {other}=\"b\"/></template></odoo>"
            ));
        }
    }

    #[test]
    fn notes_the_specs_that_find_an_old_name_and_the_trees_added() {
        let noted = |arch: &str| {
            let v = extension(arch);
            (v.tree_paths, v.tree_specs, v.escape_specs, v.tree_elements)
        };
        let paths = (true, false, false, false);
        let specs = (false, true, false, false);
        let escapes = (false, false, true, false);
        let none = (false, false, false, false);
        for expr in [
            "//field[@name='a']/tree",
            "//tree[1]",
            "//*[local-name()='tree']",
            r#"//field[@name='a']/*[name() = "tree"]"#,
            "//*[contains(local-name(),'tree')]",
            "//*['tree'=name()]",
            "//*[name(.)='tree']",
            "//*[name ()='tree']",
            "//field/*[name()!='tree']",
        ] {
            let xpath = format!(r#"<xpath expr="{}" position="inside"/>"#, expr.replace('"', "&quot;"));
            assert_eq!(noted(&xpath), paths, "{expr}");
        }
        for expr in ["//field[@name='tree']", "//*[name()='field']"] {
            let xpath = format!(r#"<xpath expr="{expr}" position="inside"/>"#);
            assert_eq!(noted(&xpath), none, "{expr}");
        }
        assert_eq!(noted(r#"<tree position="attributes"/>"#), specs);
        assert_eq!(
            noted(r#"<xpath expr="//span[@t-esc='a']" position="replace"/>"#),
            escapes
        );
        assert_eq!(noted(r#"<span t-esc="a" position="replace"/>"#), escapes);
        let set = |spec: &str| format!(r#"<span position="attributes">{spec}</span>"#);
        for spec in [
            r#"<attribute name="t-esc">b</attribute>"#,
            r#"<attribute name="t-field">b</attribute>"#,
            r#"<attribute name="t-out" add="b"/>"#,
            r#"<attribute name="t-out" remove="b"/>"#,
            r#"<attribute name="t-out"/>"#,
            r#"<attribute name="t-out"><!-- c -->b</attribute>"#,
        ] {
            assert_eq!(noted(&set(spec)), escapes, "{spec}");
        }
        for spec in [
            r#"<attribute name="t-raw">b</attribute>"#,
            r#"<attribute name="t-out">b</attribute>"#,
            r#"<attribute name="t-out" add="">b</attribute>"#,
            r#"<attribute name="t-out" separator=" ">b</attribute>"#,
        ] {
            assert_eq!(noted(&set(spec)), none, "{spec}");
        }
        assert_eq!(
            noted(r#"<xpath expr="//p" position="after"><tree/><span t-esc="a"/></xpath>"#),
            (false, false, false, true)
        );
        assert_eq!(noted(r#"<xpath expr="//p" position="after"><list/></xpath>"#), none);
    }
}
