export class HeirLoader {
    async load() {
        await this.orm.read("bad.generated.large", [1], []);
        await this.orm.read("side.order", [1], []);
    }
}
