from odoo import fields, models


class RvThing(models.Model):
    _inherit = "rv.thing"

    retyped_id = fields.Many2one("rv.parent")
    fallen_id = fields.Char()
    new_id = fields.Many2one("rv.parent")
    new_state = fields.Selection([("a", "Ay")])
