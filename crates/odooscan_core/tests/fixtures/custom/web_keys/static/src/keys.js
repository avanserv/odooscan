import { registry } from "@web/core/registry";

const fieldRegistry = registry.category("fields");
fieldRegistry.add("web_keys_stars", {}).add("web_keys_badge", {});
registry.category("view_widgets").add("web_keys_banner", {});
registry.category("actions").add("web_keys.board", {}).add("web_keys.chart", {});
