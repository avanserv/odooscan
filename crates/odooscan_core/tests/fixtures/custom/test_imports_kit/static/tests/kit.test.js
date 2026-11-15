import { expect, test } from "@odoo/hoot";
import { mountKit } from "test_imports.kit";

test("the kit mounts", () => {
    expect(mountKit(1)).toBe(1);
});
