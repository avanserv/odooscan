from odoo import fields, models


class ResPartner(models.Model):
    _inherit = "res.partner"

    gadget_weight = fields.Float()
