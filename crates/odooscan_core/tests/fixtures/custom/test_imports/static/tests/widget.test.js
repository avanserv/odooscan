import { expect, test } from "@odoo/hoot";
import { animationFrame } from "@web/../lib/hoot/hoot";
import { mountGone, onRpc } from "@web/../tests/web_test_helpers";
import { gone } from "@web/../tests/gone_helpers";
import { click } from "@web/../tests/helpers/utils";
import { clearMemoizeCaches, debounceGone } from "@web/core/utils/functions";
import { patch } from "@web/core/utils/patch";
import { mountWidget } from "./helpers";
import { mountWidget as mountAgain } from "@test_imports/../tests/helpers.js";
import { Widget } from "../src/widget";
import { _OriginalIndexedDB } from "@web/core/utils/indexed_db";

class MockedWidget extends Widget {
    static template = "test_imports.Mocked";

    async start() {
        await this.orm.call("mock.partner", "_mock_private");
        return this.orm.call("mock.partner", "x");
    }
}

patch(Widget.prototype, {
    prepare() {},
    stop() {
        return super.stop();
    },
});

test("the widget starts on the mock server", async () => {
    onRpc("/web/dataset/call_kw/orphan.hook/_compute_hook_stale", () => true);
    const widget = mountWidget(MockedWidget);
    await animationFrame();
    click(widget);
    clearMemoizeCaches();
    expect(await widget.start()).toBe(gone);
    mountGone();
    mountAgain(MockedWidget);
});
