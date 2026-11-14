import { registry } from "@web/core/registry";

const actionRegistry = registry.category("actions");

export function addLazyAction(tag) {
    actionRegistry.add(tag, {});
}
