export class OutsideLoader {
    async load() {
        await this.orm.read("bad.generated.large", [1], []);
    }
}
