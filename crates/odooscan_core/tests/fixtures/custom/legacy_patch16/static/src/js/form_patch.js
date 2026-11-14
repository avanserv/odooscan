/** @odoo-module **/
import { patch } from "@web/core/utils/patch";
import { FormController } from "@web/views/form/form_controller";

patch(FormController.prototype, "legacy_patch16.form", {
    setup() {
        this._super(...arguments);
        this.saved = false;
    },
    save: function () {
        const done = () => this._super.apply(this, arguments);
        return done();
    },
    reloadLegacy() {
        return this._super();
    },
    discardLegacy() {
        this.saved = false;
        return this._super.apply(this, arguments);
    },
    onSaved() {
        function later() {
            return this._super();
        }
        return later;
    },
    markSaved() {
        this.saved = true;
    },
});
