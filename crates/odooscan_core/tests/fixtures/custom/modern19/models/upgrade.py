from odoo import models
from odoo.osv import expression


class ResPartnerUpgrade(models.Model):
    _inherit = "res.partner"

    def _x_domain(self):
        return expression.AND([[("name", "=", "x")], [("country_id", "=", False)]])
