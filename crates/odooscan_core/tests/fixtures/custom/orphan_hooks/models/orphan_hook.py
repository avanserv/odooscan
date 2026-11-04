from odoo import api, fields, models


class OrphanHook(models.Model):
    _name = "orphan.hook"
    _description = "Orphan hook"

    name = fields.Char()
    total = fields.Integer(compute="_compute_hook_total")

    @api.depends("name")
    def _compute_hook_total(self):
        for hook in self:
            hook.total = len(hook.name or "")

    # A new model's hooks are its own to leave unused.
    def _compute_hook_spare(self):
        return None

    def _onchange_hook_parent(self):
        return None
