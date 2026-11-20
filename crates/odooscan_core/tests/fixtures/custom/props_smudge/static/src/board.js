import { Component } from "@odoo/owl";

export class Card extends Component {
    static template = "props_smudge.Card";
    static props = { title: String };
}

export class Board extends Component {
    static template = "props_smudge.Board";
    static components = { Card };
}
