from odoo import fields, models


class BadgeSealChar(models.Model):
    _inherit = "x.badge"

    seal = fields.Char(string="S")
