from odoo import fields, models


class ResCompany(models.Model):
    _inherit = "res.company"

    short_name = fields.Char()
