import { Thread } from "@mail/thread";
import { gone } from "@web/core/gone";
import { FormController, Missing } from "@web/views/form/form_controller";

export class Sites extends Thread {
    async load() {
        await this.orm.read("project.project", [1], []);
        await this.orm.read("ghost.enterprise", [1], []);
        await this.orm.call("res.partner", "action_from_enterprise", []);
        return [gone, FormController, Missing];
    }
}
