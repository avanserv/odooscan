/** @odoo-module ignore **/
odoo.define("legacy_require17.implicit", function (require) {
    "use strict";

    return require("@web/implicit_gone");
});

odoo.define("legacy_require17.listed", ["@web/listed_gone", "@web/legacy/registry"], function (require) {
    "use strict";

    return require("@web/listed_gone");
});
