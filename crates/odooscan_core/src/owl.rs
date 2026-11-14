//! Owl templates in a bundle's XML files, which the web client registers by `t-name` and
//! extends by `t-inherit`.

use std::sync::LazyLock;

use regex::Regex;
use roxmltree::{Document, Node, ParsingOptions};

use crate::facts::{ArchElement, ComponentTag, Lines, OwlTemplate, Spanned, TemplateFacts, ViewFact};
use crate::xml::{spec_tag, t_call};

/// An xpath step that names its element by an attribute, as in `div[@t-ref='x']`, or by classes,
/// as in `div[hasclass('a', 'b')]`.
static XPATH_STEP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"([A-Za-z_][\w.-]*|\*)\[(?:@([\w:.-]+)=['"]([^'"]*)['"]|hasclass\(([^)]*)\))\]"#).unwrap()
});

/// The attribute name under which an element's class tokens are recorded, as `hasclass` tests them.
const HASCLASS: &str = "hasclass";

/// Any tag, for xpath steps written `*[...]`.
const ANY: &str = "*";

/// A file that does not parse is skipped: Odoo reads it with a recovering parser, so a strict
/// error would not be Odoo's.
pub fn analyze(text: &str) -> TemplateFacts {
    let lines = Lines::new(text);
    let opts = ParsingOptions {
        allow_dtd: true,
        ..ParsingOptions::default()
    };
    let Ok(doc) = Document::parse_with_options(text, opts) else {
        return TemplateFacts::default();
    };
    let root = doc.root_element();
    // A file holds its templates under a `<templates>` or `<odoo>` root, or is a single template.
    let templates = if matches!(root.tag_name().name(), "templates" | "template" | "odoo") {
        root.children()
            .filter(Node::is_element)
            .filter_map(|node| template(node, &lines))
            .collect()
    } else {
        template(root, &lines).into_iter().collect()
    };
    TemplateFacts { templates }
}

/// A template with `t-inherit-mode="extension"` modifies its parent in place and registers no
/// name; any other needs a `t-name`, and without a mode a `t-inherit` makes it a primary child.
fn template(node: Node, lines: &Lines) -> Option<OwlTemplate> {
    let inherit = node.attribute_node("t-inherit").map(|a| Spanned {
        value: a.value().to_owned(),
        pos: lines.pos(a.range_value().start),
    });
    let extension = inherit.is_some() && node.attribute("t-inherit-mode") == Some("extension");
    let name = if extension {
        None
    } else {
        Some(node.attribute("t-name")?.to_owned())
    };
    let mut arch = ViewFact {
        id: name.clone(),
        inherit: inherit.as_ref().map(|i| i.value.clone()),
        has_arch: true,
        ..ViewFact::default()
    };
    // An extension's root only carries its own directives, which the parent never receives.
    if !extension {
        elements(node, &mut arch);
    }
    let body = node.descendants().skip(usize::from(extension));
    arch.calls.extend(body.filter_map(|n| t_call(n, lines)));
    if inherit.is_none() {
        let body: Vec<_> = node.descendants().skip(1).filter(Node::is_element).collect();
        for &n in &body {
            elements(n, &mut arch);
        }
        let components = body.into_iter().filter_map(|n| component_tag(n, lines)).collect();
        return Some(OwlTemplate {
            name,
            inherit,
            arch,
            components,
            sets_attributes: false,
        });
    }
    // An inheriting template's children are specs; what they hold is content, where an Owl
    // component may well take a `position` prop.
    let specs: Vec<_> = node.children().filter(Node::is_element).collect();
    for &spec in &specs {
        spec_facts(spec, lines, &mut arch);
    }
    let content = specs.iter().flat_map(|s| s.descendants().skip(1));
    Some(OwlTemplate {
        name,
        inherit,
        arch,
        components: content.filter_map(|n| component_tag(n, lines)).collect(),
        sets_attributes: specs.iter().any(|s| s.attribute("position") == Some("attributes")),
    })
}

/// The element as Owl compiles a component: a tag not starting lowercase, unless `t-component`
/// makes it a dynamic one (18.0 `addons/web/static/lib/owl/owl.js:5495-5536`).
fn component_tag(node: Node, lines: &Lines) -> Option<ComponentTag> {
    let tag = node.tag_name().name();
    if !node.is_element() || tag.starts_with(char::is_lowercase) || node.has_attribute("t-component") {
        return None;
    }
    let props = node
        .attributes()
        .filter(|a| a.namespace().is_none() && !a.name().starts_with("t-"))
        .filter_map(|a| {
            Some(Spanned {
                value: prop_name(a.name())?.to_owned(),
                pos: lines.pos(a.range_qname().start),
            })
        })
        .collect();
    Some(ComponentTag {
        name: Spanned {
            value: tag.to_owned(),
            pos: lines.pos(node.range().start + 1),
        },
        props,
        spread: node.has_attribute("t-props"),
        content: node.has_children(),
    })
}

/// The prop an attribute passes: the text before its first dot, as Owl reads only the next part
/// as the suffix (`owl.js:4751-4771`). None for a suffix Owl rejects when compiling the template.
fn prop_name(attr: &str) -> Option<&str> {
    let mut parts = attr.split('.');
    let name = parts.next()?;
    match parts.next() {
        None | Some("bind" | "translate" | "alike") => Some(name),
        Some(_) => None,
    }
}

/// What a spec locates, and the elements its content adds.
fn spec_facts(spec: Node, lines: &Lines, arch: &mut ViewFact) {
    locate(spec, lines, arch);
    if spec.attribute("position") == Some("attributes") {
        let set = spec.children().filter(|c| c.has_tag_name("attribute"));
        arch.elements.extend(set.flat_map(attribute_spec));
        return;
    }
    for n in spec.descendants().skip(1).filter(Node::is_element) {
        if n.has_tag_name("xpath") && n.attribute("position") == Some("move") {
            locate(n, lines, arch);
        } else {
            elements(n, arch);
        }
    }
}

/// Records what a spec must find. A spec other than an xpath matches every attribute it carries
/// but `position`, so checking the first is a necessary condition.
fn locate(spec: Node, lines: &Lines, arch: &mut ViewFact) {
    let pos = lines.pos(spec.range().start);
    let tag = spec.tag_name().name();
    let found = if tag == "xpath" {
        xpath_locators(spec.attribute("expr").unwrap_or_default())
    } else {
        let first = spec.attributes().find(|a| a.name() != "position");
        first.map(|a| element(tag, a.name(), a.value())).into_iter().collect()
    };
    arch.locators
        .extend(found.into_iter().map(|value| Spanned { value, pos }));
}

/// Records an element by each of its attributes, and by each of its classes as `hasclass` tests them.
fn elements(node: Node, arch: &mut ViewFact) {
    let tag = node.tag_name().name();
    for a in node.attributes() {
        arch.elements.extend(any_tag(element(tag, a.name(), a.value())));
        if a.name() == "class" {
            for token in a.value().split_whitespace() {
                arch.elements.extend(any_tag(element(tag, HASCLASS, token)));
            }
        }
    }
}

fn element(tag: &str, attr: &str, value: &str) -> ArchElement {
    ArchElement {
        tag: tag.to_owned(),
        attr: attr.to_owned(),
        value: value.to_owned(),
    }
}

/// The element as its tag locates it, and as `*` does.
fn any_tag(e: ArchElement) -> [ArchElement; 2] {
    let any = ArchElement {
        tag: ANY.to_owned(),
        ..e.clone()
    };
    [e, any]
}

/// What an `<attribute name="x">` spec gives the element its parent spec locates: the value it
/// sets, and for `class` each token it sets or adds.
fn attribute_spec(node: Node) -> Vec<ArchElement> {
    let (Some(attr), Some(tag)) = (node.attribute("name"), node.parent_element().and_then(spec_tag)) else {
        return vec![];
    };
    let set = node.text().map(str::trim).filter(|t| !t.is_empty());
    let mut found: Vec<ArchElement> = set.iter().flat_map(|v| any_tag(element(tag, attr, v))).collect();
    if attr == "class" {
        let tokens = set
            .into_iter()
            .chain(node.attribute("add"))
            .flat_map(str::split_whitespace);
        found.extend(tokens.flat_map(|t| any_tag(element(tag, HASCLASS, t))));
    }
    found
}

/// The steps of an xpath that name their element, each of which must match some element. A union
/// or an `or` would make a step optional, so such expressions are not checked.
fn xpath_locators(expr: &str) -> Vec<ArchElement> {
    if expr.contains('|') || expr.contains(" or ") {
        return vec![];
    }
    let mut found = vec![];
    for c in XPATH_STEP.captures_iter(expr) {
        let tag = &c[1];
        match (c.get(2), c.get(3), c.get(4)) {
            (Some(attr), Some(value), _) => found.push(element(tag, attr.as_str(), value.as_str())),
            (_, _, Some(classes)) => found.extend(
                classes
                    .as_str()
                    .split(',')
                    // An argument holding several classes matches them as a substring of the
                    // attribute, which needs each of them.
                    .flat_map(|c| c.trim().trim_matches(['"', '\'']).split_whitespace())
                    .map(|c| element(tag, HASCLASS, c)),
            ),
            _ => {}
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn show(e: &ArchElement) -> String {
        format!("{}[{}={}]", e.tag, e.attr, e.value)
    }

    #[test]
    fn tells_registered_templates_from_extensions() {
        let facts = analyze(
            r#"<?xml version="1.0"?>
<templates>
    <t t-name="x.A"><div class="o_a o_b" t-ref="root"/><Dropdown position="'bottom'"/></t>
    <t t-name="x.B" t-inherit="x.A"><xpath expr="//div[hasclass('o_a')]" position="inside"/></t>
    <t t-name="ignored" t-inherit="x.A" t-inherit-mode="extension">
        <div t-ref="root" position="after"><span class="o_c"/></div>
    </t>
    <t t-inherit="x.A" t-inherit-mode="primary"/>
</templates>"#,
        );
        let t = &facts.templates;
        assert_eq!(t.len(), 3);
        assert_eq!(t[0].name.as_deref(), Some("x.A"));
        assert_eq!(t[1].name.as_deref(), Some("x.B"));
        assert_eq!(t[2].name, None);
        let inherit = t[2].inherit.as_ref().unwrap();
        assert_eq!(
            (inherit.value.as_str(), inherit.pos.line, inherit.pos.col),
            ("x.A", 5, 36)
        );
        let elements: Vec<_> = t[0].arch.elements.iter().map(show).collect();
        assert!(elements.contains(&"div[hasclass=o_b]".to_owned()));
        assert!(elements.contains(&"*[t-ref=root]".to_owned()));
        assert!(elements.contains(&"t[t-name=x.A]".to_owned()));
        assert!(t[0].arch.locators.is_empty(), "a component's position prop is no spec");
        assert!(elements.contains(&"Dropdown[position='bottom']".to_owned()));
        let locators: Vec<_> = t[2].arch.locators.iter().map(|l| show(&l.value)).collect();
        assert_eq!(locators, ["div[t-ref=root]"]);
        // The extension's own root is not part of the parent.
        let added: Vec<_> = t[2].arch.elements.iter().map(show).collect();
        assert!(!added.iter().any(|e| e.contains("t-inherit")));
        assert!(added.contains(&"span[hasclass=o_c]".to_owned()));
    }

    #[test]
    fn reads_the_calls_of_a_template_and_of_its_specs() {
        let facts = analyze(
            r#"<templates>
    <t t-name="x.A" t-call="x.Root"><t t-call="x.B"/><t t-call="{{ dynamic }}"/></t>
    <t t-inherit="x.A" t-inherit-mode="extension" t-call="x.Dropped">
        <xpath expr="//t" position="inside"><t t-call="x.C"/></xpath>
        <xpath expr="//t" position="attributes"><attribute name="t-call">x.D</attribute></xpath>
    </t>
    <t t-name="x.Child" t-inherit="x.A" t-call="x.Kept"/>
</templates>"#,
        );
        let calls = |t: &OwlTemplate| t.arch.calls.iter().map(|c| c.value.clone()).collect::<Vec<_>>();
        assert_eq!(calls(&facts.templates[0]), ["x.Root", "x.B"]);
        assert_eq!(calls(&facts.templates[1]), ["x.C", "x.D"]);
        assert_eq!(calls(&facts.templates[2]), ["x.Kept"]);
    }

    #[test]
    fn a_spec_locates_a_call_and_its_content_calls() {
        let facts = analyze(
            r#"<templates>
    <t t-inherit="x.A" t-inherit-mode="extension">
        <t t-call="x.Old" position="replace"><t t-call="x.New" position="'top'"/></t>
        <t t-call="x.Bare"/>
    </t>
    <t t-name="x.Plain"><t t-call="x.Kept" position="'top'"/></t>
</templates>"#,
        );
        let calls = |t: &OwlTemplate| t.arch.calls.iter().map(|c| c.value.clone()).collect::<Vec<_>>();
        assert_eq!(calls(&facts.templates[0]), ["x.New"]);
        assert_eq!(calls(&facts.templates[1]), ["x.Kept"]);
        let locators: Vec<_> = facts.templates[0]
            .arch
            .locators
            .iter()
            .map(|l| show(&l.value))
            .collect();
        assert_eq!(locators, ["t[t-call=x.Old]", "t[t-call=x.Bare]"]);
    }

    #[test]
    fn reads_the_component_tags_and_their_props() {
        let facts = analyze(
            r#"<templates>
    <t t-name="x.A">
        <div><Card named="'x'" onClose.bind="close" label.translate="Hi" t-if="show" t-on-click="go"/></div>
        <Card t-props="props">
        </Card>
        <t t-component="dynamic" title="1"/>
        <Panel_x.Item size.weird="1" extra.bind.x="go"/>
    </t>
    <t t-inherit="x.A" t-inherit-mode="extension">
        <Card position="after"><Badge count="1"/></Card>
        <xpath expr="//Card" position="attributes"><attribute name="title">1</attribute></xpath>
    </t>
</templates>"#,
        );
        let shown = |t: &OwlTemplate| -> Vec<String> {
            t.components
                .iter()
                .map(|c| {
                    let props: Vec<_> = c.props.iter().map(|p| format!("{}@{}", p.value, p.pos.col)).collect();
                    format!(
                        "{}@{}:{} [{}]{}{}",
                        c.name.value,
                        c.name.pos.line,
                        c.name.pos.col,
                        props.join(" "),
                        if c.spread { " spread" } else { "" },
                        if c.content { " content" } else { "" }
                    )
                })
                .collect()
        };
        let t = &facts.templates;
        assert_eq!(
            shown(&t[0]),
            [
                "Card@3:15 [named@20 onClose@32 label@53]",
                "Card@4:10 [] spread content",
                "Panel_x.Item@7:10 [extra@38]"
            ]
        );
        assert!(!t[0].sets_attributes);
        assert_eq!(shown(&t[1]), ["Badge@10:33 [count@39]"]);
        assert!(t[1].sets_attributes);
    }

    #[test]
    fn reads_a_file_that_is_one_template() {
        let facts = analyze(r#"<t t-name="x.Single"><span class="o_s"/></t>"#);
        assert_eq!(facts.templates.len(), 1);
        assert_eq!(facts.templates[0].name.as_deref(), Some("x.Single"));
    }

    #[test]
    fn reads_the_named_steps_of_an_xpath() {
        let shown = |expr| xpath_locators(expr).iter().map(show).collect::<Vec<_>>();
        assert_eq!(
            shown("//div[hasclass('o_a', \"o_b o_c\")]/*[@t-ref='x']//Dropdown[@position='bottom']"),
            [
                "div[hasclass=o_a]",
                "div[hasclass=o_b]",
                "div[hasclass=o_c]",
                "*[t-ref=x]",
                "Dropdown[position=bottom]"
            ]
        );
        assert!(shown("//div[@name='a'] | //div[@name='b']").is_empty());
        assert!(shown("//div[contains(@class, 'o_a')]").is_empty());
    }

    #[test]
    fn attribute_specs_add_what_they_set() {
        let facts = analyze(
            r#"<templates><t t-inherit="x.A" t-inherit-mode="extension">
    <xpath expr="//div[@t-ref='root']" position="attributes">
        <attribute name="class" add="o_d" separator=" "/>
        <attribute name="t-ref">main</attribute>
    </xpath>
</t></templates>"#,
        );
        let added: Vec<_> = facts.templates[0].arch.elements.iter().map(show).collect();
        assert!(added.contains(&"div[hasclass=o_d]".to_owned()));
        assert!(added.contains(&"div[t-ref=main]".to_owned()));
    }
}
