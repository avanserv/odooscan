export class OwnerCalls {
    async load() {
        await this.orm.searchRead("rpc.owner.lamp", [], ["name"]);
        await this.orm.searchRead("rpc.owner.mast", [], ["name"]);
    }
}
