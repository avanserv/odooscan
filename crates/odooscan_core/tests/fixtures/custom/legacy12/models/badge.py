from odoo import fields, models


class LegacyTag(models.AbstractModel):
    _name = "legacy.tag"
    _description = "Legacy tag"
    _inherits = {"res.partner": "partner_id"}

    partner_id = fields.Many2one("res.partner", required=True)
    country_id = fields.Many2one("res.country", delegate=True, required=True)
