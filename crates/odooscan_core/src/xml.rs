use std::sync::LazyLock;

use regex::Regex;
use roxmltree::{Document, Node, ParsingOptions};

use crate::facts::{
    AccessRow, ArchElement, ArchField, DataFacts, Lines, Pos, RawDiag, RecordFact, Spanned, ViewFact, XmlDef, XmlRef,
};

static EVAL_REF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"\bref\(\s*['"]([\w.]+)['"]"#).unwrap());
/// A field step followed by more steps, as in `//field[@name='line_ids']//field[...]`, reaches into
/// the subview of line_ids.
static XPATH_CONTAINER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"field\[@name=['"](\w+)['"]\]/+([\w*.-]+)(::)?"#).unwrap());
/// An xpath ending on a field step locates that field wherever it is, subviews included.
static XPATH_LAST_FIELD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"field\[@name=['"](\w+)['"]\]$"#).unwrap());
/// A step on a view tag other than through a field may land in the main view or in any subview.
static XPATH_AMBIGUOUS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(^|/)(list|tree|kanban)\b").unwrap());

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

/// Tags under which a `<field>` opens a subview on the field's comodel.
const SUBVIEWS: &[&str] = &["list", "tree", "form", "kanban", "graph", "pivot", "calendar", "search"];

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
                for cap in EVAL_REF.captures_iter(eval).filter(|c| !c[1].ends_with('.')) {
                    self.reference(&cap[1], n);
                }
            }
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
                "act_window" | "report" | "asset" => {
                    self.def(child);
                    self.groups(child);
                }
                "function" => self.eval_refs(child),
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
            ..ViewFact::default()
        };
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
        self.facts.records.push(record);
    }

    fn record_field(&mut self, field: Node, name: &str, model: &str, view: &mut ViewFact) {
        if let Some(id) = field.attribute("ref") {
            self.reference(id, field);
            if name == "model_id" && matches!(model, "ir.model.access" | "ir.access") {
                self.facts.access.push(AccessRow {
                    model: id.to_owned(),
                    by_name: false,
                });
            }
        }
        self.eval_refs(field);
        if model == "ir.ui.view" {
            self.view_field(field, name, view);
        }
        if name == "view_mode" && field.text().is_some_and(|t| t.split(',').any(|m| m.trim() == "tree")) {
            let pos = self.pos(field);
            self.facts
                .diags
                .push(RawDiag::new("OD306", pos, "view_mode 'tree' is renamed 'list'"));
        }
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
            "arch" | "arch_db" => {
                view.has_arch = true;
                for root in field.children().filter(Node::is_element) {
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
        self.arch_attrs(node, pos);
        self.groups(node);
        self.arch_tag(node, tag, pos);
        if tag == "field"
            && let Some(name) = node.attribute("name").filter(|n| !n.contains('.'))
        {
            self.arch_field(node, name, pos, path, view);
            return;
        }
        let depth = path.len();
        match tag {
            "xpath" => xpath_steps(node.attribute("expr").unwrap_or_default(), path),
            "groupby" => path.extend(node.attribute("name").map(str::to_owned)),
            _ => {}
        }
        for child in node.children().filter(Node::is_element) {
            self.arch(child, path, view);
        }
        path.truncate(depth);
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
        view.fields.push(ArchField {
            path: path.iter().cloned().chain([name.to_owned()]).collect(),
            pos,
        });
        for child in node.children().filter(Node::is_element) {
            if SUBVIEWS.contains(&child.tag_name().name()) {
                path.push(name.to_owned());
                self.arch(child, path, view);
                path.pop();
            } else {
                self.arch(child, path, view);
            }
        }
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
            attr,
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
    let spec = node.parent_element()?;
    let tag = match spec.tag_name().name() {
        "xpath" => {
            let step = spec.attribute("expr")?.rsplit('/').next()?;
            let step = step.rsplit("::").next()?;
            step.split('[').next()?
        }
        tag => tag,
    };
    Some(ArchElement {
        tag: tag.to_owned(),
        attr,
        value: node.text()?.trim().to_owned(),
    })
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
                attr: if &c[2] == "name" { "name" } else { "id" },
                value: c[3].to_owned(),
            },
            pos,
        })
        .collect()
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
