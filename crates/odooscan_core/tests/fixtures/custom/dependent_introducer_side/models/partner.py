from odoo import fields, models


class Partner(models.Model):
    _inherit = "res.partner"

    lamp_shade = fields.Char()


class Base(models.AbstractModel):
    _inherit = "base"

    lamp_watts = fields.Integer()
