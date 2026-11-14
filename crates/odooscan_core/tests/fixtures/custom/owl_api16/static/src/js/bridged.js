/** @odoo-module **/
const { Component } = owl;

export class Bridged extends Component {
    save() {
        this.trigger("saved");
    }
}
