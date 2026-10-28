import { patch } from "@web/core/utils/patch";
import { FormController } from "@web/views";

export class QuickForm extends FormController {
    quick() {}
}

patch(QuickForm.prototype, {
    archive() {},
});

patch(QuickForm.prototype, {
    setup() {
        super.setup();
        super.render();
        super.save();
        super.archive();
        super.quick();
    },
    discard() {
        return super.discard();
    },
});
