from odoo import fields, models


class LegacyBadge(models.AbstractModel):
    _name = "legacy.badge"
    _description = "Legacy badge"
    _inherits = {"res.partner": "partner_id"}

    partner_id = fields.Many2one("res.partner", required=True, ondelete="set null")
    country_id = fields.Many2one("res.country", delegate=True, required=True, ondelete="set null")
    id = fields.Integer(readonly=True)
    ref = fields.Char(string="ID")
