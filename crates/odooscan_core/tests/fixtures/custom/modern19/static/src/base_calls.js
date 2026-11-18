export class PartnerBaseCalls {
    async load() {
        await this.orm.call("res.partner", "get_metadata", [[1]]);
        await this.orm.call("res.partner", "is_transient", []);
        await this.orm.call("res.partner", "ids", []);
        await this.orm.call("res.partner", "x_tier_count", []);
        await this.orm.call("res.partner", "x_tier_scale", [2]);
        await this.orm.call("res.partner", "x_tier_label", ["gold"]);
        await this.orm.call("res.partner", "x_tier_mark", [[1]]);
        await this.orm.call("res.partner", "x_tier_open", [[1]]);
        await this.orm.call("res.partner", "x_tier_kind", [[1]]);
    }
}
