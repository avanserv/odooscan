import { patch } from "@web/core/utils/patch";
import { Component } from "@odoo/owl";
import { FormController } from "@web/views";

export class LedgerForm extends FormController {}

patch(LedgerForm.prototype, {
    discard() {
        return super.discard();
    },
});

export class LedgerCard extends Component {
    static template = "encoded_lost_code.Gone";

    async load() {
        await this.orm.call("encoded.ledger", "action_gone", []);
    }
}
