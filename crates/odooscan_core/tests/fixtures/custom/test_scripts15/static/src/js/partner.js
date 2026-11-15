odoo.define("test_scripts15.partner", function (require) {
    "use strict";
    const Widget = require("web.Widget");
    return Widget.extend({
        load() {
            console.log("loading");
            return this._rpc({model: "x.missing", method: "_secret"});
        },
    });
});
