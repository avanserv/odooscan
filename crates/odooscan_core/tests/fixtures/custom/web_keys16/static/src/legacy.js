/** @odoo-module **/
import { useService } from "@web/core/utils/hooks";

export function useLegacy() {
    return useService("web_keys16_bridged");
}
