odoo.define("legacy_require14.rpc", function (require) {
    "use strict";

    const Widget = require("web.Widget");

    return Widget.extend({
        start() {
            this.$el.html(this.value);
            return this._rpc({model: "x.gone14", method: "read"});
        },
    });
});
