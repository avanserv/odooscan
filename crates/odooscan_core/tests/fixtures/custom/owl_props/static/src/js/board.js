import { Component } from "@odoo/owl";
import { Badge, BigCard, Card, Chip, FancyCard, Framed, ListCard, Loose, Pair, Patchy, Peg, Tile } from "@owl_props/js/card";
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

export class Desk extends Component {
    static template = "owl_props.Desk";
    static components = { Tile, Badge, Chip, Card };
}

export class Rack extends Component {
    static template = "owl_props.Rack";
    static components = { Card, Hook: Peg };
}
