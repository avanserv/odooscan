from odoo import fields, models


class ResPartner(models.Model):
    _inherit = "res.partner"

    side_thing = fields.Char(related="x_thing")
