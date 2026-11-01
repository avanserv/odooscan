from odoo import models
from odoo.http import request


class SoftPartner(models.Model):
    _inherit = "res.partner"

    def _things(self):
        return self.env["soft.thing"].search([("code", "=", "no guard")])

    def _behind_hasattr(self):
        if hasattr(self, "soft_mark"):
            return self.env["soft.thing"].search([("code", "=", "a")])

    def _behind_installed(self):
        if "soft_guards_side" in self.env["ir.module.module"]._installed():
            return self.env["soft.thing"].search([("code", "=", "b")])

    def _behind_get(self):
        if self.env["ir.module.module"]._get("soft_guards_side").state == "installed":
            return self.env["soft.thing"].search([("code", "=", "c")])

    def _behind_search(self):
        modules = self.env["ir.module.module"]
        if modules.search([("name", "=", "soft_guards_side"), ("state", "=", "installed")]):
            return self.env["soft.thing"].search([("code", "=", "d")])

    def _after_field_test(self):
        if "code" not in self.env["soft.thing"]:
            return None
        return self.env["soft.thing"].search([("code", "=", "e")])

    def _local_holder(self):
        installed = "soft_guards_side" in self.env["ir.module.module"]._installed()
        if installed:
            return self.env["soft.thing"].search([("code", "=", "f")])

    def _two_step_holder(self):
        installed = "soft_guards_side" in self.env["ir.module.module"]._installed()
        usable = installed and self.active
        if usable:
            return self.env["soft.thing"].search([("code", "=", "g")])

    def _behind_super_hasattr(self):
        if hasattr(super(), "soft_mark"):
            return self.env["soft.thing"].search([("code", "=", "super hasattr")])

    def _behind_named_hasattr(self, name):
        if hasattr(self, name):
            return self.env["soft.thing"].search([("code", "=", "name hasattr")])

    def _holder_of_other(self):
        usable = self.active
        if usable:
            return self.env["soft.thing"].search([("code", "=", "other holder")])

    def _before_the_guard(self):
        things = self.env["soft.thing"].search([("code", "=", "before")])
        if "soft.thing" not in self.env:
            return None
        return things

    def _in_the_else(self):
        if "soft.thing" in self.env:
            return None
        else:
            return self.env["soft.thing"].search([("code", "=", "else")])

    def _behind_a_model_lookup(self):
        if self.env["ir.model"]._get("soft.thing"):
            return self.env["soft.thing"].search([("code", "=", "model lookup")])

    def _behind_a_none_test(self):
        if self.env.get("soft.thing") is not None:
            return self.env["soft.thing"].search([("code", "=", "h")])

    def _after_the_registry(self):
        if "soft.thing" not in self.env.registry:
            return None
        return self.env["soft.thing"].search([("code", "=", "i")])

    def _in_the_else_of_a_negated_guard(self):
        if not hasattr(self, "soft_mark"):
            return None
        else:
            return self.env["soft.thing"].search([("code", "=", "j")])

    def _after_a_negated_holder(self):
        missing = "soft.thing" not in self.env
        if missing:
            return None
        return self.env["soft.thing"].search([("code", "=", "k")])

    def _holder_replaced_after_its_use(self):
        usable = "soft.thing" in self.env
        if usable:
            self.env["soft.thing"].search([("code", "=", "l")])
        usable = self.active
        return usable

    def _left_when_present(self):
        if "soft.thing" in self.env:
            return None
        return self.env["soft.thing"].search([("code", "=", "inverted return")])

    def _left_on_two_tests(self):
        if self.active and "code" in self.env["soft.thing"]:
            return None
        return self.env["soft.thing"].search([("code", "=", "and leaves")])

    def _behind_a_negated_guard(self):
        if not hasattr(self, "soft_mark"):
            return self.env["soft.thing"].search([("code", "=", "negated")])

    def _behind_a_negated_holder(self):
        missing = "soft.thing" not in self.env
        if missing:
            return self.env["soft.thing"].search([("code", "=", "negated holder")])

    def _holder_assigned_after_its_use(self):
        usable = self.active
        if usable:
            self.env["soft.thing"].search([("code", "=", "assigned after")])
        usable = "soft.thing" in self.env
        return usable

    def _holder_replaced_before_its_use(self):
        usable = "soft.thing" in self.env
        usable = self.active
        if usable:
            return self.env["soft.thing"].search([("code", "=", "replaced before")])

    def _behind_a_variable_field(self, name):
        field = self._fields.get(name)
        if not field:
            raise ValueError(name)
        return self.env["soft.thing"].search([("code", "=", "variable field")])

    def _behind_a_comprehension(self, names):
        usable = [name for name in names if "soft_mark" in self._fields]
        if usable:
            return self.env["soft.thing"].search([("code", "=", "comprehension")])

    def _behind_request_hasattr(self):
        if hasattr(request, "soft_mark"):
            return self.env["soft.thing"].search([("code", "=", "request hasattr")])

    def _behind_private_hasattr(self):
        if hasattr(self, "_soft_mark"):
            return self.env["soft.thing"].search([("code", "=", "private hasattr")])

    def _after_a_state_set(self):
        side = self.env["ir.module.module"]._get("soft_guards_side")
        if side.state in ("uninstalled", "to remove"):
            return None
        return self.env["soft.thing"].search([("code", "=", "m")])

    def _behind_a_module_record(self):
        if self.env["ir.module.module"]._get("soft_guards_side"):
            return self.env["soft.thing"].search([("code", "=", "module record")])

    def _behind_a_search_of_any_state(self):
        if self.env["ir.module.module"].search([("name", "=", "soft_guards_side")]):
            return self.env["soft.thing"].search([("code", "=", "any state")])

    def _behind_a_search_of_the_absent(self):
        modules = self.env["ir.module.module"]
        if modules.search([("name", "=", "soft_guards_side"), ("state", "=", "uninstalled")]):
            return self.env["soft.thing"].search([("code", "=", "absent")])

    def _behind_a_state_on_its_way_out(self):
        side = self.env["ir.module.module"]._get("soft_guards_side")
        if side.state in ("uninstalled", "to remove"):
            return self.env["soft.thing"].search([("code", "=", "way out")])

    def _behind_a_record_and_its_absent_state(self):
        side = self.env["ir.module.module"]._get("soft_guards_side")
        if side and side.state == "uninstalled":
            return self.env["soft.thing"].search([("code", "=", "record and state")])
