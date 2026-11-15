export class Dropped {
    async load() {
        await this.orm.call("res.partner", "_bundle_dropped", []);
    }
}
