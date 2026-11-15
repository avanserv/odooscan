export class Frontend {
    async load() {
        await this.orm.call("res.partner", "_bundle_frontend", []);
    }
}
