from odoo import fields, models


class ResPartner(models.Model):
    _inherit = "res.partner"

    name = fields.Char(required=True)
