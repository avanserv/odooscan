export class Hooks {
    async load() {
        await this.orm.call("button.plain", "action_hook", []);
        await this.orm.call("button.ghosted", "action_hook", []);
    }
}
