/** @odoo-module **/
import { patch } from "@web/core/utils/patch";
import { Bridged } from "@owl_api16/js/bridged";

patch(Bridged.prototype, "owl_api16.bridged", {
    trigger(name, payload) {
        this.env.bus.trigger(name, payload);
    },
});
