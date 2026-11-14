odoo.define("legacy_require14.counter", function (require) {
    "use strict";

    var registry = require("@web/legacy/registry");
    var zoom = require("web.zoomodoo");
    var gone = require("@web/gone");
    var tools = require("legacy_require_lib.tools");

    return { registry: registry, zoom: zoom, gone: gone, tools: tools };
});
