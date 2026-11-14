export class OldPanel {
    async load() {
        await this.orm.call("res.partner", "_bundle_old_panel", []);
    }
}
