/** @odoo-module **/
import { tags } from "@odoo/owl";

const { Component, hooks, QWeb: Q } = owl;
const { useState } = owl.hooks;

export class Panel extends Component {
    setup() {
        this.state = useState({ open: false });
    }

    toggle() {
        this.trigger("toggled", { open: this.state.open });
    }
}

export class Header extends owl.Component {
    onClick = () => this.trigger("clicked");
}

export const unused = [hooks, Q, tags, owl.tags.xml, owl.QWeb];
