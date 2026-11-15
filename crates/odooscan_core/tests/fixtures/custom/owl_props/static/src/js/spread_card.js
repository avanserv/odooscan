import { Component } from "@odoo/owl";
import { standardCardProps } from "@owl_props/js/standard";

export class SpreadCard extends Component {
    static template = "owl_props.Card";
    static props = { ...standardCardProps };
}
