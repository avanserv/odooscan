import { Component } from "@odoo/owl";

export class Card extends Component {
    static template = "owl_props16.Card";
    static props = { title: String };
}

export class Board extends Component {
    static template = "owl_props16.Board";
    static components = { Card };
}
