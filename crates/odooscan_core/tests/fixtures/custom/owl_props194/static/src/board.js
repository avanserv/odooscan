import { Component } from "@odoo/owl";

export class Card extends Component {
    static template = "owl_props194.Card";
    static props = { title: String };
}

export class Board extends Component {
    static template = "owl_props194.Board";
    static components = { Card };
}
