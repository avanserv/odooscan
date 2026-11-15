import { Component } from "@odoo/owl";

export class Card extends Component {
    static template = "owl_props.Card";
    static props = {
        title: String,
        subtitle: { type: String, optional: true },
        onClose: { type: Function, optional: true },
    };
}

export class BigCard extends Card {}

export class FancyCard extends Component {
    static template = "owl_props.Card";
    static props = { ...Card.props, tone: String };
}

export class ListCard extends Component {
    static template = "owl_props.Card";
    static props = ["title", "subtitle?"];
}

export class Pair extends Component {
    static template = "owl_props.Card";
    static props = { left: String, right: String, gap: { type: Number, optional: true } };
}

export class Loose extends Component {
    static template = "owl_props.Card";
    static props = ["*"];
}

export class Framed extends Component {
    static template = "owl_props.Card";
    static props = { slots: Object };
}

export class Patchy extends Component {
    static template = "owl_props.Card";
    static props = { title: String };
}
