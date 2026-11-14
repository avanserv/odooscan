odoo.define("legacy_rpc16.partner_widget_tests", function () {
    "use strict";

    QUnit.test("load", async function (assert) {
        await this._rpc({model: "x.mocked", method: "_mocked_helper"});
        assert.ok(true);
    });
});
