from odoo import models


class LostHook(models.Model):
    _inherit = "lost.hook"

    # Silent: the script beside it is not UTF-8, and names it in ASCII all the same.
    def _compute_hook_unread(self):
        return None

    # Reported: the script that is not UTF-8 names no such method.
    def _compute_hook_unnamed(self):
        return None
