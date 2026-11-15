export class NewPanel {
    async load() {
        await this.orm.call("res.partner", "_bundle_new_panel", []);
    }
}
