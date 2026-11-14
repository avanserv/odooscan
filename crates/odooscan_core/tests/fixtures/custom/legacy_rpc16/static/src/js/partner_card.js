/** @odoo-module **/

export class PartnerCard {
    load() {
        return this._rpc({model: "res.partner", method: "action_card_gone"});
    }
}
