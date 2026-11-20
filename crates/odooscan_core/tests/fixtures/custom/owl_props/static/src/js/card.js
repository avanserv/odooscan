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

export class Tile extends Component {
    static template = "owl_props.Card";
    static props = { title: String };
}

export class TileAlias extends Component {
    static template = "owl_props.Card";
    static props = Tile.props;
}

export class Badge extends Component {
    static template = "owl_props.Card";
    static props = { title: String };
}

export class BadgeCopy extends Component {
    static template = "owl_props.Card";
    static props = { ...Badge.props };
}

export class Chip extends Component {
    static template = "owl_props.Card";
    static props = { title: String };
}

export class ChipCopy extends Component {
    static template = "owl_props.Card";
    static props = { ...Chip.props };
}

export class Peg extends Component {
    static template = "owl_props.Card";
    static props = { title: String };
}
