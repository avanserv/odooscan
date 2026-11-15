export class Shared {
    async load() {
        await this.orm.call("res.partner", "_bundle_shared", []);
    }
}
