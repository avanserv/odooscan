import { Component } from "@odoo/owl";

export class Widget extends Component {
    static template = "test_imports.Gone";

    start() {
        return true;
    }
}
