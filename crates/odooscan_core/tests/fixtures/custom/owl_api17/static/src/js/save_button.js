import { Component } from "@odoo/owl";
import { FormController } from "@web/views/form/form_controller";

export class SaveButton extends Component {
    onClick() {
        this.trigger('save');
    }
}

export class NoteController extends FormController {
    discard() {
        this.trigger("discarded");
    }
}
