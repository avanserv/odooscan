odoo.define("test_scripts15.partner_tests", function () {
    "use strict";

    QUnit.test("load", async function (assert) {
        const partner = {
            _rpc: async () => true,
            async load() {
                console.log("loading");
                await this._rpc({model: "x.missing", method: "_secret"});
            },
        };
        await partner.load();
        assert.ok(true);
    });
});
