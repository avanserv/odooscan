/** @odoo-module **/
import { patch } from "@web/core/utils/patch";
import { Header, Panel } from "@owl_api15/js/panel";

patch(Panel.prototype, "owl_api15.panel", {
    mounted() {
        this._super(...arguments);
        this.state.open = true;
    },
    willUnmount() {
        this._super(...arguments);
    },
});

patch(Header.prototype, "owl_api15.header", {
    async willStart() {
        await this._super(...arguments);
    },
});
