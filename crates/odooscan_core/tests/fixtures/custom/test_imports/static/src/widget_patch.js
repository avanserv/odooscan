import { patch } from "@web/core/utils/patch";
import { Widget } from "./widget";

// Reported: the test's patch that adds prepare() applies only while the test runs.
patch(Widget.prototype, {
    start() {
        return super.prepare();
    },
});
