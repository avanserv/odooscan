from odoo import fields, models


class Partner(models.Model):
    _inherit = "res.partner"

    lamp_shade = fields.Char()
