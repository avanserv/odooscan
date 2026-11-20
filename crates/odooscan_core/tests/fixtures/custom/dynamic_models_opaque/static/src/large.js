export class OpaqueLoader {
    async load() {
        await this.orm.read("bad.generated.large", [1], []);
        await this.orm.read("ghost.opaque", [1], []);
    }
}
