export class PlainLoader {
    async load() {
        await this.orm.read("bad.generated.large", [1], []);
    }
}
