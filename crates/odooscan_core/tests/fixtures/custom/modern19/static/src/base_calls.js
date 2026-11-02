export class PartnerBaseCalls {
    async load() {
        await this.orm.call("res.partner", "get_metadata", [[1]]);
        await this.orm.call("res.partner", "is_transient", []);
        await this.orm.call("res.partner", "ids", []);
    }
}
