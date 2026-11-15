/** @odoo-module ignore **/
odoo.define("web_keys.legacy", ["@web/core/utils/hooks"], function (require) {
    const { useService } = require("@web/core/utils/hooks");
    return () => useService("web_keys_pager");
});
