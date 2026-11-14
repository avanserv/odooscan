/** @odoo-module **/
import { patch } from "@web/core/utils/patch";
import { FormController } from "@web/views/form/form_controller";

patch(FormController.prototype, "legacy_patch144.form", {
    reloadEarly() {
        return this._super();
    },
});
