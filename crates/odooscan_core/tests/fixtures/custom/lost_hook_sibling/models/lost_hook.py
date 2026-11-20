from odoo import models


class LostHook(models.Model):
    _inherit = "lost.hook"

    # Silent: the script of orphan_hooks_lost names it past a byte that is not UTF-8, and a name
    # any indexed module spells counts.
    def _compute_hook_beside(self):
        return None
