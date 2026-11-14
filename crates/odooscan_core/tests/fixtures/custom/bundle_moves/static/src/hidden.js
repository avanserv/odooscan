export class Hidden {
    async load() {
        await this.orm.call("res.partner", "_bundle_hidden", []);
    }
}
