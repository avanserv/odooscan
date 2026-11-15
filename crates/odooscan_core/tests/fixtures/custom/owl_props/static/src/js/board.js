import { Component } from "@odoo/owl";
import { BigCard, Card, FancyCard, Framed, ListCard, Loose, Pair, Patchy } from "@owl_props/js/card";
import { SpreadCard } from "@owl_props/js/spread_card";

export class Board extends Component {
    static template = "owl_props.Board";
    static components = { Card, Big: BigCard, Fancy: FancyCard, ListCard, Pair, Loose, Framed, Patchy, SpreadCard };
}

export class TallBoard extends Board {}

export class Panel extends Component {
    static template = "owl_props.Panel";
    static components = { Card };
}

export class Shelf extends Component {
    static template = "owl_props.Shelf";
    static components = { Card };
}
