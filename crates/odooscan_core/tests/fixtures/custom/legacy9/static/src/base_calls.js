/** @odoo-module **/

export class PartnerBaseCalls {
    async load() {
        await this.orm.call("res.partner", "get_metadata", [[1]]);
        await this.orm.call("res.partner", "get_formview_id", [[1]]);
        await this.orm.call("res.partner", "load_views", [[]]);
        await this.orm.call("res.partner", "concat", [[1]]);
        await this.orm.call("res.partner", "ids", []);
    }
}
