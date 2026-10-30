from odoo import fields, models


class SaasPass(models.AbstractModel):
    _name = "saas.pass"
    _description = "SaaS pass"

    country_id = fields.Many2one("res.country", delegate=True, required=True)
