odoo.define("owl_api15.legacy_panel", function () {
    "use strict";

    const { Component } = owl;

    class LegacyPanel extends Component {
        close() {
            this.trigger("closed");
        }
    }

    return LegacyPanel;
});
