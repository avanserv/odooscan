from odoo import models


class MixedHook(models.Model):
    _inherit = "mixed.hook"

    # Reported: the plain base of the model's class is read, and defines no such method.
    def _onchange_hook_mixed(self):
        return None

    # Silent: the plain base of the model's class defines it.
    def _compute_hook_step(self):
        return None


class HiddenHook(models.Model):
    _inherit = "hidden.hook"

    # Silent: the plain base of the model's class is imported, and the index keeps no imports.
    def _onchange_hook_hidden(self):
        return None
