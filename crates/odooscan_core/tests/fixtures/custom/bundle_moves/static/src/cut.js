export class Cut {
    async load() {
        await this.orm.call("res.partner", "_bundle_cut", []);
    }
}
