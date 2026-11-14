odoo.define("legacy_rpc16.partner_widget", function (require) {
    "use strict";

    const rpc = require("web.rpc");

    return {
        load(options) {
            const self = this;
            this._rpc({model: "x.missing", method: "read"});
            self._rpc({"model": "res.partner", "method": "action_gone"});
            rpc.query({model: "res.partner", method: "_compute_secret"});
            this._rpc({model: this.modelName, method: "_hidden_helper"});
            this._rpc({route: "/legacy_rpc16/data", model: "x.routed", method: "read"});
            this._rpc({...options, model: "x.spread", method: "read"});
            this._rpc({model: "x.no_method", args: []});
            this._rpc({model: "res.partner", method: "read", args: [[1]]});
            options.other.query({model: "x.other", method: "read"});
        },
    };
});
