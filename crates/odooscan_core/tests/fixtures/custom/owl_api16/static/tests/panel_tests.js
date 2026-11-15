/** @odoo-module **/
import { tags } from "@odoo/owl";

const { Component, hooks } = owl;

export class TestPanel extends Component {
    setup() {
        owl.hooks.useState({});
        this.trigger("ready");
    }
}

export const unused = [hooks, tags, owl.QWeb];
