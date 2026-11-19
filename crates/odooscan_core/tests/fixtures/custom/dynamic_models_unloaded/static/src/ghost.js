export class UnloadedLoader {
    async load() {
        await this.orm.read("ghost.unloaded", [1], []);
    }
}
