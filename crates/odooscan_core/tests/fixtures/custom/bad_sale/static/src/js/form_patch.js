import { Component } from "@odoo/owl";
import { patch } from "@web/core/utils/patch";
import { FormController, Missing } from "@web/views/form/form_controller";
import { FormController as Indexed } from "@web/views";
import { Thread } from "@mail/thread";
import { gone } from "@web/core/gone";
import legacy from "@web/legacy/registry";
import { helper } from "./helpers";
import helpers from "./helpers";
// odooscan: ignore[OD601]
import { Moved } from "@web/views/form/form_controller";

patch(Missing.prototype, {
    setup() {
        super.setup();
    },
});

patch(FormController.prototype, {
    save() {
        helper(Indexed, Thread, gone, legacy, helpers, Moved, Component);
        return super.save();
    },
});
