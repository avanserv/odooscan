import { patch } from "@web/core/utils/patch";
import { Panel } from "@web/core/panel";
import { FormController } from "@web/views/form/form_controller";

export class NoteController extends FormController {
    discard() {
        this.trigger("discarded");
    }
}

export class NotePanel extends Panel {
    close() {
        this.trigger("closed");
    }
}

patch(FormController.prototype, {
    mounted() {
        super.mounted();
    },
});

patch(Panel.prototype, {
    willStart() {
        return super.willStart();
    },
});
