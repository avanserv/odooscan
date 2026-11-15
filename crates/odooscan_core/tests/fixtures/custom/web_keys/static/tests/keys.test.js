import { registry } from "@web/core/registry";
import { useService } from "@web/core/utils/hooks";

registry.category("fields").add("web_keys_mocked", {});
registry.category("view_widgets").add("web_keys_mocked", {});
registry.category("services").add("web_keys_mocked", {});

export function useMocked() {
    return useService("web_keys_unregistered");
}

registry.category("web_tour.tours").add("web_keys_unit_tour", { steps: () => [] });

export function hasRuler(env) {
    return "web_keys_ruler" in env.services;
}
