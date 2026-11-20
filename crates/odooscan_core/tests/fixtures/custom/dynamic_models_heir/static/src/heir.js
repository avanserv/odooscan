export class HeirLoader {
    async load() {
        await this.orm.read("bad.generated.large", [1], []);
        await this.orm.read("side.order", [1], []);
        await this.orm.read("bad.generated.tiny", [1], []);
        await this.orm.read("heir.ghost", [1], []);
        await this.orm.read("x_heir.note", [1], []);
    }
}
