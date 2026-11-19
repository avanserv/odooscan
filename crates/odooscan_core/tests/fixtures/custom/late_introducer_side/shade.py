from odoo import fields, models


class Shade(models.AbstractModel):
    _name = "late.shade"
    _description = "Shade"

    color = fields.Char()
