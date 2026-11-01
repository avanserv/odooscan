export class PartnerBaseCalls {
    async load() {
        await this.orm.call("res.partner", "get_metadata", [[1]]);
        await this.orm.call("res.partner", "check_field_access", ["name", "read"]);
        await this.orm.call("res.partner", "get_formview_action", [[1]]);
        await this.orm.call("res.partner", "ids", []);
    }
}
