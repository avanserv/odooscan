odoo.define("test_scripts15.partner", function () {
    "use strict";

    return {
        load() {
            console.log("loading");
            return this._rpc({model: "x.missing", method: "_secret"});
        },
    };
});
