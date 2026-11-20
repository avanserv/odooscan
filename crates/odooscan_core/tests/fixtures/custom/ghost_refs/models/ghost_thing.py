from odoo import fields, models


# No indexed class introduces ghost.thing, so its parents and their methods are out of sight.
class GhostThing(models.Model):
    _inherit = "ghost.thing"

    tag = fields.Char()

    def action_go(self):
        return super().action_go()

    def _compute_spare(self):
        return None

    def action_sign(self, stamp):
        return stamp


class GhostThingSign(models.Model):
    _inherit = "ghost.thing"

    def action_sign(self):
        return None
