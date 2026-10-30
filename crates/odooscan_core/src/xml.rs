use std::sync::LazyLock;

use regex::Regex;
use roxmltree::{Document, Node, ParsingOptions};

use crate::domain;
use crate::facts::{
    AccessRow, ActionFact, ActionValue, ArchButton, ArchDomain, ArchElement, ArchField, ArchModifier, DataFacts,
    DomainFact, FunctionCall, Lines, Pos, RawDiag, RecordFact, RuleFact, ServerFact, Spanned, ViewFact, XmlDef, XmlRef,
};
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
/// The key of a context that makes Odoo take a view for a type by its id.
static CONTEXT_VIEW_REF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\w+)_view_ref\b").unwrap());
/// The type of a row an `eval` writes to `view_ids`, as a dict literal or `dict()` gives it.
static VIEW_ROW_MODE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\bview_mode['"]?\s*[:=]\s*['"](\w+)['"]"#).unwrap());
/// A field step followed by more steps, as in `//field[@name='line_ids']//field[...]`, reaches into
/// the subview of line_ids.
static XPATH_CONTAINER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"field\[@name=['"](\w+)['"]\]/+([\w*.-]+)(::)?"#).unwrap());
/// An xpath ending on a field step locates that field wherever it is, subviews included.
static XPATH_LAST_FIELD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"field\[@name=['"](\w+)['"]\]$"#).unwrap());
/// A step on a view tag other than through a field may land in the main view or in any subview.
static XPATH_AMBIGUOUS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(^|/)(list|tree|kanban|column)\b").unwrap());

/// Path segment for an arch position whose model cannot be told without the parent view.
pub const UNKNOWN_CONTEXT: &str = "?";
/// Prefix of a path segment naming a `<field position="...">` locator rather than a subview.
pub const LOCATOR: char = '~';
/// An xpath step that names its element, as in `field[@name='x']` or `div[@id="x"]`.
static XPATH_NAMED_STEP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"([A-Za-z][\w-]*)\[@(name|id)=['"]([^'"]+)['"]\]"#).unwrap());
/// Attributes by which a spec names the element it locates.
const LOCATING_ATTRS: [&str; 2] = ["name", "id"];
static ACTION_REF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"%\(([\w.]+)\)d").unwrap());

/// Attributes that make the value of a `<field>` something other than its `eval` or its text.
const INDIRECT_VALUE: [&str; 4] = ["ref", "search", "file", "type"];

/// Tags under which a `<field>` opens a subview on the field's comodel.
const SUBVIEWS: &[&str] = &["list", "tree", "form", "kanban", "graph", "pivot", "calendar", "search"];

/// Attributes Odoo reads as Python expressions on the records of an arch.
const MODIFIERS: [&str; 4] = ["column_invisible", "invisible", "readonly", "required"];

/// Tags a spec may locate in the main view or in the subview of any x2many field.
const AMBIGUOUS_SPECS: &[&str] = &["list", "tree", "kanban", "column"];

const CRON: &str = "ir.cron";

/// What opens a CDATA section, which a text node's range starts on.
const CDATA: &str = "<![CDATA[";

/// The view type of a `<template>`, and the type a `t-call` target must have on the server.
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

pub fn analyze(text: &str) -> DataFacts {
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
            facts: &mut facts,
        }
        .data(root);
    }
    facts
}

struct Walker<'a> {
    lines: &'a Lines,
    facts: &'a mut DataFacts,
}

impl Walker<'_> {
    fn pos(&self, node: Node) -> Pos {
        self.lines.pos(node.range().start)
    }

    fn def(&mut self, node: Node) {
        if let Some(id) = node.attribute("id") {
            let pos = self.pos(node);
            let model = node.attribute("model").map(str::to_owned);
            self.facts.defs.push(XmlDef {
                id: id.to_owned(),
                model,
                pos,
                assigned: false,
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
                    assigned: true,
                });
            }
        }
    }

    fn function_call(&mut self, node: Node) {
        if let (Some(model), Some(method)) = (node.attribute("model"), attr_value(node, "name", self.lines)) {
            self.facts.functions.push(FunctionCall {
                model: model.to_owned(),
                method,
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
                    self.def(child);
                    if let Some(parent) = child.attribute("inherit_id") {
                        self.reference(parent, child);
                    }
                    self.template(child);
                    self.groups(child);
                    self.unsafe_qweb(child);
                }
                "menuitem" => self.menuitem(child),
                "act_window" => {
                    self.def(child);
                    self.groups(child);
                    self.act_window(child);
                }
                "report" | "asset" => {
                    self.def(child);
                    self.groups(child);
                }
                "function" => {
                    self.update_xmlids(child);
                    self.eval_refs(child);
                    self.function_call(child);
                }
                _ => {}
            }
        }
    }

    /// A `<template>` is a QWeb view whose arch is its content.
    fn template(&mut self, node: Node) {
        let mut view = ViewFact {
            id: node.attribute("id").map(str::to_owned),
            inherit: node.attribute("inherit_id").map(str::to_owned),
            has_arch: true,
            kind: Some(QWEB.to_owned()),
            ..ViewFact::default()
        };
        self.facts.view_keys.extend(node.attribute("key").map(str::to_owned));
        // The loader turns the `<template>` itself into the arch's root, which keeps its `t-call`.
        view.calls
            .extend(node.descendants().filter_map(|n| t_call(n, self.lines)));
        for n in node.descendants().skip(1).filter(Node::is_element) {
            let pos = self.pos(n);
            arch_element(n, pos, &mut view);
        }
        self.facts.views.push(view);
    }

    fn menuitem(&mut self, node: Node) {
        self.def(node);
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
        self.def(node);
        let Some(model) = node.attribute("model") else { return };
        let mut record = RecordFact {
            model: Spanned {
                value: model.to_owned(),
                pos: self.pos(node),
            },
            fields: vec![],
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
            self.facts.views.push(view);
        }
        if model == "ir.actions.act_window" {
            self.action_domain(node);
            self.action(node);
        }
        if model == "ir.actions.act_window.view" {
            self.action_view(node);
        }
        if let Some(domain) = rule_domain_field(model) {
            self.rule(node, domain);
        }
        if matches!(model, "ir.actions.server" | CRON) {
            self.server(node, model == CRON);
        }
        self.facts.records.push(record);
    }

    /// Odoo takes an `eval` over the text next to it, and only a run tells what that gives.
    fn action(&mut self, node: Node) {
        let field = |name: &str| field_of(node, name);
        let written = |name: &str| match field(name) {
            None => ActionValue::Unset,
            Some(f) if f.has_attribute("eval") => ActionValue::Opaque,
            Some(f) => literal(spec_text(f, self.lines)),
        };
        let mut action = ActionFact {
            id: node.attribute("id").map(str::to_owned),
            model: written("res_model"),
            view_mode: written("view_mode"),
            view_of_any_kind: field("view_id").is_some_and(names_a_view),
            view_kinds: vec![],
        };
        if let Some(f) = field("context") {
            action.view_kinds = context_view_kinds(f.attribute("eval").or_else(|| f.text()));
        }
        if let Some(eval) = field("view_ids").and_then(|f| f.attribute("eval")) {
            let rows: Vec<String> = VIEW_ROW_MODE.captures_iter(eval).map(|c| c[1].to_owned()).collect();
            action.view_of_any_kind |= rows.is_empty();
            action.view_kinds.extend(rows);
        }
        for row in field("view_ids").iter().flat_map(|f| f.children()).filter(is_view_row) {
            name_row_view(&mut action, row);
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
        self.facts.rules.push(RuleFact {
            id: node.attribute("id").map(str::to_owned),
            model,
            by_name,
            fields: domain::written_fields(&domain),
            domain,
            active: field_of(node, "active").map(rule_active),
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
            state: last("state").map(|f| !indirect_value(f) && f.text() == Some("code")),
            writes_code: code.is_some(),
            refused,
            calls,
        });
    }

    /// Reads `field` as Odoo does, which validates the stripped text up to the first child: why it
    /// refuses the code, and the calls of code it accepts.
    fn server_code(&self, field: Node) -> (Option<Spanned<String>>, Vec<Spanned<String>>) {
        let Some(text) = field.first_child().filter(|t| t.is_text() && !indirect_value(field)) else {
            return (None, vec![]);
        };
        let raw = text.text().unwrap_or_default();
        let lead = raw.len() - raw.trim_start().len();
        let read = server_code::read(raw.trim());
        let spanned = |(value, at): (String, usize)| Spanned {
            value,
            pos: code_pos(text, raw, lead + at, self.lines),
        };
        (read.error.map(spanned), read.calls.into_iter().map(spanned).collect())
    }

    /// The tag Odoo reads through 16.0. It writes `view_mode` even without the attribute, as Odoo
    /// then gives the default.
    fn act_window(&mut self, node: Node) {
        let written = |name: &str| literal(attr_value(node, name, self.lines));
        let action = ActionFact {
            id: node.attribute("id").map(str::to_owned),
            model: written("res_model"),
            view_mode: written("view_mode"),
            view_kinds: context_view_kinds(node.attribute("context")),
            view_of_any_kind: node.attribute("view_id").is_some_and(|id| !id.is_empty()),
        };
        self.facts.actions.push(action);
    }

    /// A row of the `view_ids` of the action its `act_window_id` names. Odoo overwrites that field on
    /// a row nested in `view_ids`, so such a row belongs to the enclosing action, which `action` reads.
    fn action_view(&mut self, node: Node) {
        if node.parent_element().is_some_and(|p| p.has_tag_name("field")) {
            return;
        }
        let action = field_of(node, "act_window_id").and_then(|f| f.attribute("ref"));
        let Some(action) = action.filter(|id| !id.is_empty()) else {
            return;
        };
        let mut row = ActionFact {
            id: Some(action.to_owned()),
            ..ActionFact::default()
        };
        name_row_view(&mut row, node);
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
            let pos = self.pos(field);
            self.facts
                .diags
                .push(RawDiag::new("OD306", pos, "view_mode 'tree' is renamed 'list'"));
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
                global_write: false,
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
            "inherit_id" => view.inherit = field.attribute("ref").map(str::to_owned),
            // A `type` wins over the arch's root wherever the record sets it.
            "type" => view.kind = field.text().map(|t| t.trim().to_owned()),
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
        view.subviews.extend(spec_subview(tag, path));
        view.calls.extend(t_call(node, self.lines));
        self.arch_button(node, path, view);
        self.arch_modifiers(node, path, view);
        self.arch_attrs(node, pos);
        self.groups(node);
        self.arch_tag(node, tag, pos);
        if tag == "field"
            && let Some(name) = node.attribute("name").filter(|n| !n.contains('.'))
        {
            self.arch_field(node, name, pos, path, view);
            return;
        }
        if tag == "filter" {
            let fields = domain_attr(node, "domain").chain(group_by_attr(node));
            arch_domain(pos, path.clone(), fields, view);
        }
        let depth = path.len();
        match tag {
            "xpath" => xpath_steps(node.attribute("expr").unwrap_or_default(), path),
            _ if AMBIGUOUS_SPECS.contains(&tag) && node.has_attribute("position") => {
                path.push(UNKNOWN_CONTEXT.to_owned())
            }
            "groupby" => {
                path.extend(node.attribute("name").map(str::to_owned));
                view.subviews.extend((path.len() > depth).then(|| path.clone()));
            }
            _ => {}
        }
        for child in node.children().filter(Node::is_element) {
            self.arch(child, path, view);
        }
        path.truncate(depth);
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
        let parsed = found.into_iter().filter(|(_, value)| !constant_modifier(&value.value));
        view.modifiers.extend(parsed.map(|(attr, value)| ArchModifier {
            path: path.to_vec(),
            attr,
            value,
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

    fn arch_tag(&mut self, node: Node, tag: &str, pos: Pos) {
        match tag {
            "tree" => self
                .facts
                .diags
                .push(RawDiag::new("OD306", pos, "the <tree> view is renamed <list>")),
            "attribute" if matches!(node.attribute("name"), Some("attrs" | "states")) => {
                self.facts.diags.push(RawDiag::new("OD305", pos, "the attrs and states attributes are removed; write invisible, readonly or required as a Python expression"));
            }
            _ => {}
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
        arch_domain(pos, field_path.clone(), domain_attr(node, "domain"), view);
        arch_domain(pos, path.clone(), domain_attr(node, "filter_domain"), view);
        view.fields.push(ArchField { path: field_path, pos });
        for child in node.children().filter(Node::is_element) {
            if SUBVIEWS.contains(&child.tag_name().name()) {
                path.push(name.to_owned());
                view.subviews.push(path.clone());
                self.arch(child, path, view);
                path.pop();
            } else {
                self.arch(child, path, view);
            }
        }
    }
}

fn field_of<'a, 'i>(node: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    node.children()
        .find(|c| c.has_tag_name("field") && c.attribute("name") == Some(name))
}

/// The field Odoo keeps of those of a name: each one replaces the value of the one before.
fn last_field_of<'a, 'i>(node: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    node.children()
        .rfind(|c| c.has_tag_name("field") && c.attribute("name") == Some(name))
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

/// What a `model_id` field writes, as Odoo reads it: a `search`, a `ref`, then an `eval`. With
/// `true` the value is a model name, else an XML id.
fn rule_model(field: Node, lines: &Lines) -> (ActionValue, bool) {
    let opaque = (ActionValue::Opaque, false);
    if field.has_attribute("use") || field.attribute("model").is_some_and(|m| m != "ir.model") {
        return opaque;
    }
    if let Some(search) = attr_value(field, "search", lines).filter(|s| !s.value.is_empty()) {
        return domain::searched_model(&search.value).map_or(opaque, |value| {
            let pos = search.pos;
            (ActionValue::Literal(Spanned { value, pos }), true)
        });
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

/// Only an `eval` or the text tells a domain without a run.
fn rule_domain(field: Node, lines: &Lines) -> ActionValue {
    if INDIRECT_VALUE.iter().any(|a| field.has_attribute(*a)) {
        return ActionValue::Opaque;
    }
    literal(attr_value(field, "eval", lines).or_else(|| spec_text(field, lines)))
}

/// An `eval` that is no constant counts as false. Odoo tests the text of a boolean untrimmed, so
/// anything but these three spellings, an empty field too, is true.
fn rule_active(field: Node) -> bool {
    if INDIRECT_VALUE.iter().any(|a| field.has_attribute(*a)) {
        return false;
    }
    match field.attribute("eval") {
        Some(eval) => matches!(eval.trim(), "True" | "1"),
        None => {
            let text = field.text().unwrap_or_default().to_lowercase();
            !matches!(text.as_str(), "0" | "false" | "off")
        }
    }
}

/// The value of a field of an action that is present: a blank one holds no literal.
pub fn literal(text: Option<Spanned<String>>) -> ActionValue {
    text.filter(|t| !t.value.trim().is_empty())
        .map_or(ActionValue::Opaque, ActionValue::Literal)
}

/// Whether a `view_id` field holds a view: Odoo reads an empty ref as no value, and modules clear
/// the field with `eval="False"`.
fn names_a_view(field: Node) -> bool {
    field.attribute("ref").is_some_and(|id| !id.is_empty())
        || field
            .attribute("eval")
            .is_some_and(|e| !matches!(e.trim(), "" | "False" | "None"))
}

fn is_view_row(node: &Node) -> bool {
    node.has_tag_name("record") && node.attribute("model") == Some("ir.actions.act_window.view")
}

/// Adds what a row of `view_ids` names to `action`. A row without a view only orders the type, so
/// Odoo still looks a view up for it.
fn name_row_view(action: &mut ActionFact, row: Node) {
    if !field_of(row, "view_id").is_some_and(names_a_view) {
        return;
    }
    let kind = field_of(row, "view_mode").and_then(|f| f.text()).map(str::trim);
    match kind.filter(|k| !k.is_empty()) {
        Some(kind) => action.view_kinds.push(kind.to_owned()),
        None => action.view_of_any_kind = true,
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

/// The expression an `<attribute>` spec gives a modifier: its text, or an `add`, which Odoo joins to
/// the value it finds by `and` or `or`, so it reads alone. Any other separator splits the value at
/// places only that value tells.
fn spec_modifier(node: Node, lines: &Lines) -> Option<(&'static str, Spanned<String>)> {
    let attr = MODIFIERS.into_iter().find(|a| node.attribute("name") == Some(a))?;
    let joined = matches!(node.attribute("separator").map(str::trim), Some("and" | "or"));
    let value = match node.has_attribute("add") {
        true if joined => attr_value(node, "add", lines)?,
        true => return None,
        false => spec_text(node, lines)?,
    };
    Some((attr, value))
}

/// Empty attributes are no expression, and Odoo takes these before it parses, in any case.
fn constant_modifier(value: &str) -> bool {
    value.is_empty()
        || ["0", "1", "true", "false"]
            .iter()
            .any(|c| value.eq_ignore_ascii_case(c))
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

fn arch_domain(pos: Pos, path: Vec<String>, fields: impl Iterator<Item = String>, view: &mut ViewFact) {
    let fields: Vec<Spanned<String>> = fields.map(|value| Spanned { value, pos }).collect();
    if !fields.is_empty() {
        view.domains.push(ArchDomain { path, fields });
    }
}

/// Records `node` as a locator when it is a spec, or else as an element views extending this one
/// can locate. A spec on another tag than field matches every attribute it carries, so checking one
/// that names the element is a necessary condition.
fn arch_element(node: Node, pos: Pos, view: &mut ViewFact) {
    let tag = node.tag_name().name();
    let named = LOCATING_ATTRS
        .iter()
        .filter_map(|&attr| node.attribute(attr).map(|v| (attr, v)))
        .map(|(attr, v)| ArchElement {
            tag: tag.to_owned(),
            attr: attr.to_owned(),
            value: v.to_owned(),
        });
    match tag {
        "xpath" => view
            .locators
            .extend(xpath_locators(node.attribute("expr").unwrap_or_default(), pos)),
        // An attribute spec can rename the element it locates; any other attribute it sets is no element.
        "attribute" => view.elements.extend(renamed(node)),
        _ if node.has_attribute("position") => view.locators.extend(named.take(1).map(|value| Spanned { value, pos })),
        _ => view.elements.extend(named),
    }
}

/// The element an `<attribute name="name">` spec gives a new name to.
fn renamed(node: Node) -> Option<ArchElement> {
    let attr = LOCATING_ATTRS.into_iter().find(|a| node.attribute("name") == Some(a))?;
    Some(ArchElement {
        tag: spec_tag(node.parent_element()?)?.to_owned(),
        attr: attr.to_owned(),
        value: node.text()?.trim().to_owned(),
    })
}

/// The template a node calls by a literal name: its `t-call` at the value, or the text of an
/// `<attribute name="t-call">` spec. An empty, numeric, formatted or path target names no template
/// a scan can look up: Odoo reads digits as a view id, and `{{ }}` and `#{}` only at render time.
pub fn t_call(node: Node, lines: &Lines) -> Option<Spanned<String>> {
    let target = if node.has_attribute("t-call") {
        attr_value(node, "t-call", lines)?
    } else if node.has_tag_name("attribute") && node.attribute("name") == Some("t-call") {
        spec_text(node, lines)?
    } else {
        return None;
    };
    literal_target(&target.value).then_some(target)
}

/// The text of a spec, trimmed, at its first character.
fn spec_text(node: Node, lines: &Lines) -> Option<Spanned<String>> {
    let text = node.first_child().filter(Node::is_text)?;
    let raw = text.text()?;
    Some(Spanned {
        value: raw.trim().to_owned(),
        pos: lines.pos(text.range().start + (raw.len() - raw.trim_start().len())),
    })
}

/// Where the byte `at` of `decoded`, the value of the node `text`, sits. The node holds no offsets of
/// its own past its start, so newlines give the line, which entities and CDATA leave as they are, and
/// the column counts from the last one, short of any entity or CDATA opening earlier on the line.
fn code_pos(text: Node, decoded: &str, at: usize, lines: &Lines) -> Pos {
    let start = text.range().start;
    let first = lines.pos(start);
    let before = &decoded[..at];
    match before.rfind('\n') {
        Some(newline) => Pos {
            line: first.line + before.matches('\n').count() as u32,
            col: (at - newline) as u32,
        },
        None => {
            let opened = text.document().input_text()[start..].starts_with(CDATA);
            let skipped = if opened { CDATA.len() } else { 0 };
            Pos {
                col: first.col + (skipped + at) as u32,
                ..first
            }
        }
    }
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

/// The named steps of an xpath, each of which must match some element. A union or an `or` would
/// make a step optional, so such expressions are not checked.
fn xpath_locators(expr: &str, pos: Pos) -> Vec<Spanned<ArchElement>> {
    if expr.contains('|') || expr.contains(" or ") {
        return vec![];
    }
    XPATH_NAMED_STEP
        .captures_iter(expr)
        .map(|c| Spanned {
            value: ArchElement {
                tag: c[1].to_owned(),
                attr: c[2].to_owned(),
                value: c[3].to_owned(),
            },
            pos,
        })
        .collect()
}

/// Where a subview tag opens when `path` ends on a locator: in that field, which the spec gives a
/// subview. Fields, domains and buttons in it keep the locator's own position.
fn spec_subview(tag: &str, path: &[String]) -> Option<Vec<String>> {
    let field = path.last()?.strip_prefix(LOCATOR)?;
    let opened = path.iter().map(String::as_str).chain([field]);
    SUBVIEWS.contains(&tag).then(|| opened.map(str::to_owned).collect())
}

/// Extends `path` with where an xpath's target sits: the subviews it reaches into and the field it
/// locates.
fn xpath_steps(expr: &str, path: &mut Vec<String>) {
    let before = path.len();
    // Axes such as `/..` or `/following-sibling::x` stay in the field's own view.
    let containers = XPATH_CONTAINER
        .captures_iter(expr)
        .filter(|c| c.get(3).is_none() && &c[2] != "..");
    path.extend(containers.map(|c| c[1].to_owned()));
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

    #[test]
    fn a_template_root_calls_too() {
        let facts = analyze(r#"<odoo><template id="x" t-call="a.b"/></odoo>"#);
        assert_eq!(facts.views[0].calls[0].value, "a.b");
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
            [(vec!["~a".to_owned()], "invisible", "b".to_owned(), 3, 3)]
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

    fn subviews(arch: &str) -> Vec<Vec<String>> {
        let facts = analyze(&format!(
            r#"<odoo><record id="x" model="ir.ui.view"><field name="arch" type="xml">{arch}</field></record></odoo>"#
        ));
        facts.views[0].subviews.clone()
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
        assert_eq!(
            subviews(r#"<field name="a" position="after"><field name="b"/></field>"#),
            [[""; 0]; 0]
        );
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
        assert_eq!(named(r#"<field name="view_id" ref="v"/>"#), (vec![], true));
        assert_eq!(named(r#"<field name="view_id" eval="ref('v')"/>"#), (vec![], true));
        assert_eq!(named(r#"<field name="view_id" eval="False"/>"#), none);
        assert_eq!(named(r#"<field name="view_id" ref=""/>"#), none);
        assert_eq!(named(r#"<field name="view_id"/>"#), none);
        assert_eq!(
            named(r#"<field name="context">{'gantt_view_ref': 'm.v', 'x': 1}</field>"#),
            kinds(&["gantt"])
        );
        assert_eq!(
            named(r#"<field name="context" eval="dict(map_view_ref='m.v')"/>"#),
            kinds(&["map"])
        );
        assert_eq!(
            named(
                r#"<field name="view_ids" eval="[(5, 0, 0), (0, 0, {'view_mode': 'gantt', 'view_id': ref('v')}), Command.create(dict(view_mode='map'))]"/>"#
            ),
            kinds(&["gantt", "map"])
        );
        assert_eq!(
            named(r#"<field name="view_ids" eval="[(6, 0, [ref('row')])]"/>"#),
            (vec![], true)
        );
        assert_eq!(named(r#"<field name="view_ids"/>"#), none);
    }

    #[test]
    fn reads_the_views_an_act_window_tag_names() {
        let action = action_facts(r#"<act_window id="a" view_id="v" context="{'map_view_ref': 'm.v'}"/>"#).remove(0);
        assert_eq!(action.view_kinds, ["map"]);
        assert!(action.view_of_any_kind);
        assert!(!action_facts(r#"<act_window id="a" view_id=""/>"#)[0].view_of_any_kind);
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
        let no_view = row(r#"<field name="view_mode">gantt</field><field name="act_window_id" ref="a"/>"#);
        assert!(action_facts(&no_view).is_empty());
        let no_action = row(r#"<field name="view_mode">gantt</field><field name="view_id" ref="v"/>"#);
        assert!(action_facts(&no_action).is_empty());
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
        assert_eq!(actions[0].view_kinds, ["gantt"]);
        assert!(!actions[0].view_of_any_kind);

        let hidden = nested(r#"<field name="view_mode" eval="'gantt'"/><field name="view_id" ref="v"/>"#);
        assert!(hidden[0].view_of_any_kind);

        let no_view = nested(r#"<field name="view_mode">gantt</field>"#);
        assert!(no_view[0].view_kinds.is_empty());
        assert!(!no_view[0].view_of_any_kind);

        let elsewhere = nested(
            r#"<field name="view_mode">gantt</field><field name="view_id" ref="v"/><field name="act_window_id" ref="b"/>"#,
        );
        assert_eq!(elsewhere.len(), 1);
        assert_eq!(elsewhere[0].id.as_deref(), Some("a"));
        assert_eq!(elsewhere[0].view_kinds, ["gantt"]);
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
            r#"<field name="domain_force"/>"#,
        ] {
            let rule = rule_of(field);
            assert!(matches!(rule.domain, ActionValue::Opaque), "{field}");
            assert!(rule.fields.is_empty(), "{field}");
        }
        let unset = rule_of(r#"<field name="name">a</field>"#);
        assert!(matches!(unset.domain, ActionValue::Unset));
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
}
