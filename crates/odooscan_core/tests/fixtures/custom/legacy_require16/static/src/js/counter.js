odoo.define("legacy_require16.counter", function (require) {
    "use strict";

    const registry = require("@web/legacy/registry");
    const helpers = require("legacy_require16.helpers");
    const zoom = require("web.zoomodoo");
    const sibling = require("./x");
    const gone = require("@web/gone");
    const tools = require("legacy_require_lib.tools");
    const loader = { require() {} };
    loader.require("@web/loader_gone");

    return { registry, helpers, zoom, sibling, gone, tools };
});
