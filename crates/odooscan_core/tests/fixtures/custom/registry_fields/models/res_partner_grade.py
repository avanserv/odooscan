from odoo import fields, models


class ResPartnerSilver(models.Model):
    _inherit = "res.partner"

    x_grade = fields.Selection([("silver", "Silver")])
    x_tier = fields.Selection(selection_add=[("high",)], ondelete={"high": "cascade"})
