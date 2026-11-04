from odoo import models


class HookSteps:
    def _compute_hook_step(self):
        return None


class MixedHook(models.Model, HookSteps):
    _name = "mixed.hook"
    _description = "Mixed hook"
