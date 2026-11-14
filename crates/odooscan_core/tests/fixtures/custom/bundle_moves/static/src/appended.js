export class Appended {
    async load() {
        await this.orm.call("res.partner", "_bundle_appended", []);
    }
}
