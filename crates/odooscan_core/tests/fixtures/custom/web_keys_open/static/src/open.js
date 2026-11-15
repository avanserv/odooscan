import { registry } from "@web/core/registry";

export function addWidgets(prefix) {
    registry.category("fields").add(`${prefix}_gauge`, {});
    registry.category("view_widgets").add(`${prefix}_seal`, {});
}
