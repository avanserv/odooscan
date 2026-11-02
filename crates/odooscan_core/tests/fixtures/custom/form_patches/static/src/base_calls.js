export class CardBaseCalls {
    async load() {
        await this.orm.call("form.card", "get_metadata", [[1]]);
        await this.orm.call("form.card", "get_external_id", [[1]]);
        await this.orm.call("form.card", "get_property_definition", ["x_size"]);
        await this.orm.call("form.card", "is_transient", []);
        await this.orm.call("form.card", "ids", []);
        await this.orm.call("form.card", "concat", [[1]]);
        await this.orm.call("form.card", "name_get", [[1]]);
    }
}
