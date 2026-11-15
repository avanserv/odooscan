odoo.define("test_scripts15.partner_tests", function () {
    "use strict";

    QUnit.test("load", async function (assert) {
        console.log("loading");
        await this._rpc({model: "x.missing", method: "_secret"});
        assert.ok(true);
    });
});
