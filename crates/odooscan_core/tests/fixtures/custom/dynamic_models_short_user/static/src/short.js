export class ShortLoader {
    async load() {
        await this.orm.read("side.order", [1], []);
    }
}
