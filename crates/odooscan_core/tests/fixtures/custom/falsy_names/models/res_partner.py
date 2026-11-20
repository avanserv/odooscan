from odoo import fields, models


class ResPartner(models.Model):
    _name = False
    _inherit = ["res.country", "res.currency"]

    lamp_count = fields.Integer()
