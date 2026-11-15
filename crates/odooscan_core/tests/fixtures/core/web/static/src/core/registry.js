class Registry {
    category(name) {
        return this[name] || new Registry();
    }

    add(key, value) {
        this[key] = value;
        return this;
    }
}

export const registry = new Registry();

registry.category("fields").add("many2many_tags", {}).add("list.handle", {});
registry.category("view_widgets").add("web_ribbon", {});
registry.category("actions").add("reload", {});
