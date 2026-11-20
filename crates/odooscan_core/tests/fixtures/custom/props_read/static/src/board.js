import { Component } from "@odoo/owl";

export class Card extends Component {
    static template = "props_read.Card";
    static props = { title: String };
}

export class Board extends Component {
    static template = "props_read.Board";
    static components = { Card };
}
