odoo.define("owl_api16.legacy_panel", function (require) {
    "use strict";

    const { LegacyComponent } = require("@web/legacy/legacy_component");
    const { Component } = owl;

    class LegacyPanel extends Component {
        close() {
            this.trigger("closed");
        }
    }

    class Required extends LegacyComponent {
        close() {
            this.trigger("closed");
        }
    }

    return { LegacyPanel, Required };
});
