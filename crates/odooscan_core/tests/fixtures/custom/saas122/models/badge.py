from odoo import fields, models


class SaasStub(models.AbstractModel):
    _name = "saas.stub"
    _description = "SaaS stub"

    country_id = fields.Many2one("res.country", delegate=True, required=True)
    currency_id = fields.Many2one("res.currency", delegate=True, required=True, ondelete=None)
    partner_id = fields.Many2one("res.partner", delegate=True, required=True, ondelete="")
    model_id = fields.Many2one("ir.model", delegate=True, required=True)


class SaasTab(models.AbstractModel):
    _name = "saas.tab"
    _description = "SaaS tab"
    _inherits = {"res.partner": "partner_id", "res.country": "country_id"}

    partner_id = fields.Many2one("res.partner", required=True, ondelete=False)
    country_id = fields.Many2one("res.country", required=True, ondelete="")
