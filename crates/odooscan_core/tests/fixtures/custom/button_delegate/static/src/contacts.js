export class Contacts {
    async load() {
        await this.orm.call("res.users", "action_contact", []);
        await this.orm.call("res.users", "action_contcat", []);
        await this.orm.call("res.partner", "action_contact", []);
        await this.orm.call("res.partner", "action_contcat", []);
        await this.orm.call("res.partner", "action_follow", []);
        await this.orm.call("button.copied", "action_contact", []);
    }
}
