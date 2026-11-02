from odoo import api, fields, models


class Hub(models.Model):
    _name = "ring.hub"
    _inherits = {"ring.spoke": "spoke_id"}

    spoke_id = fields.Many2one("ring.spoke", required=True, ondelete="cascade")
    shown = fields.Char(compute="_compute_shown")

    @api.depends("tag")
    def _compute_shown(self):
        self.shown = False


class Spoke(models.Model):
    _name = "ring.spoke"
    _inherits = {"ring.hub": "hub_id"}

    hub_id = fields.Many2one("ring.hub", required=True, ondelete="cascade")
    tag = fields.Char(related="hub_id.tag")
