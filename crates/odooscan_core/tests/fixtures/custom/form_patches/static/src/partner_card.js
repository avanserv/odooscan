export class PartnerCard {
    async load(orm) {
        await this.orm.call("res.partner", "name_search", []);
        await this.orm.call("res.partner", "action_unfollow", []);
        await this.orm.call("res.partner", "action_follow", []);
        await orm.silent.searchRead("res.partner.title", [], []);
        await this.orm.read("mail.thread", [1], []);
        await this.orm.call(this.props.resModel, "write", []);
        await this.orm.call("res.partner", "_compute_display_name", []);
        await this.orm.call(this.props.resModel, "_onchange_partner", []);
    }
}
