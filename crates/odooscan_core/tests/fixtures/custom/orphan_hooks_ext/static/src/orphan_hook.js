export class OrphanHook {
    async load() {
        await this.orm.call("orphan.hook", "onchange_hook_rpc", []);
    }
}
