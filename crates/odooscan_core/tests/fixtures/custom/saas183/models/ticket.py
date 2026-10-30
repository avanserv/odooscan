from odoo import fields, models


class SaasTicket(models.AbstractModel):
    _name = "saas.ticket"
    _description = "SaaS ticket"
    _inherits = {"res.partner": "partner_id"}

    partner_id = fields.Many2one("res.partner", required=True, ondelete="set null")
    country_id = fields.Many2one("res.country", delegate=True, required=True, ondelete="set null")
    currency_id = fields.Many2one("res.currency", delegate=True, required=True)
