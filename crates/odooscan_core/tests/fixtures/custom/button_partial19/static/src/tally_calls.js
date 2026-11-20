export class TallyCalls {
    async load() {
        await this.orm.call("button.tally", "action_tally", []);
        await this.orm.call("button.tally", "is_transient", []);
        await this.orm.call(this.props.resModel, "is_transient", []);
    }
}
