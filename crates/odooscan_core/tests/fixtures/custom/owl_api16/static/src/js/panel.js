/** @odoo-module **/
import { EventBus, tags } from "@odoo/owl";
import { LegacyComponent } from "@web/legacy/legacy_component";

const { Component, hooks, QWeb: Q } = owl;
const { useState } = owl.hooks;

export class Panel extends Component {
    static template = owl.tags.xml`<div t-on-click="toggle"/>`;

    setup() {
        this.state = useState({ open: false });
    }

    toggle() {
        this.state.open = !this.state.open;
        this.trigger("toggled", { open: this.state.open });
    }
}

export class Header extends owl.Component {
    onClick = () => this.trigger("clicked");
}

export class Kept extends LegacyComponent {
    save() {
        this.trigger("saved");
    }
}

export class Bus extends EventBus {
    notify() {
        this.trigger("changed");
    }
}

export class Quiet extends Component {
    setup() {
        this.env.bus.trigger("ready");
    }

    later() {
        return function () {
            this.trigger("later");
        };
    }

    static make() {
        return this.trigger("made");
    }
}

export class Own extends Component {
    trigger(name) {
        this.env.bus.trigger(name);
    }

    save() {
        this.trigger("saved");
    }
}

export const unused = [hooks, Q, tags];
