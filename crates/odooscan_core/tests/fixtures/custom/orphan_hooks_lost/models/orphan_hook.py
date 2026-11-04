from odoo import models


class OrphanHook(models.Model):
    _inherit = "orphan.hook"

    # Silent: the script beside it is not UTF-8, so the scan cannot read whether it names this.
    def _compute_hook_unread(self):
        return None
