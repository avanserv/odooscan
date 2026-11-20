export class LanternLookup {
    async load(ids) {
        await this.orm.read("ghost.lantern", ids, ["wick"]);
        await this.orm.read("side.lantern", ids, ["wick"]);
        await this.orm.read("ghost.thing", ids, ["tag"]);
        await this.orm.read("res.partner", ids, ["name"]);
    }
}
