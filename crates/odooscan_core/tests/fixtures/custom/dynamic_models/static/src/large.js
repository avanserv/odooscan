export class LargeLoader {
    async load() {
        await this.orm.read("bad.generated.large", [1], []);
    }
}
