from odoo import fields, models


class ResPartner(models.Model):
    _inherit = "res.partner"

    x_grade = fields.Selection(selection_add=[("bronze",), ("silver",)])
    x_tier = fields.Selection(selection_add=[("high",)])
