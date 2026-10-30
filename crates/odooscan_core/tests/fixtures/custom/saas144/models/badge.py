from odoo import fields, models


class SaasBadge(models.AbstractModel):
    _name = "saas.badge"
    _description = "SaaS badge"
    _inherits = {"res.partner": "partner_id"}

    partner_id = fields.Many2one("res.partner", required=True, ondelete="set null")
    country_id = fields.Many2one("res.country", delegate=True, required=True, ondelete="set null")
