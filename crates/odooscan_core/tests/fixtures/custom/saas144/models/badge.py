from odoo import fields, models


class SaasBadge(models.AbstractModel):
    _name = "saas.badge"
    _description = "SaaS badge"
    _inherits = {"res.partner": "partner_id"}

    partner_id = fields.Many2one("res.partner", required=True, ondelete="set null")
    country_id = fields.Many2one("res.country", delegate=True, required=True, ondelete="set null")
    ref = fields.Char(string="ID")


class SaasBadgeHolder(models.AbstractModel):
    _name = "saas.badge.holder"
    _description = "SaaS badge holder"

    country_id = fields.Many2one("res.country", delegate=True, required=True)
