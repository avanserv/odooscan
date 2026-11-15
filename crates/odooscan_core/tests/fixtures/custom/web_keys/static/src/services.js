import { registry } from "@web/core/registry";
import { useService } from "@web/core/utils/hooks";

registry.category("services").add("web_keys_clock", { start() {} });

export function useKeys() {
    return {
        clock: useService("web_keys_clock"),
        typo: useService("web_keys_clokc"),
        mocked: useService("web_keys_mocked"),
        radio: useService("web_keys_side_radio"),
        beacon: useService("web_keys_beacon"),
        ruler: useService("web_keys_ruler"),
    };
}
