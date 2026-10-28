from odoo import fields, models


class ResPartner(models.Model):
    _inherit = "res.partner"

    home_country_id = fields.Many2one("res.country", delegate=True)
