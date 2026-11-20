import { Component } from "@odoo/owl";

export class Card extends Component {
    static template = "props_smear.Card";
    static props = { title: String };
}

export class Board extends Component {
    static template = "props_smear.Board";
    static components = { Card };
}
