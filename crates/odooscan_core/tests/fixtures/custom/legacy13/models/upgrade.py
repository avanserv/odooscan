from odoo import models


class ResPartnerUpgrade(models.Model):
    _inherit = "res.partner"

    def _x_in_company(self, company):
        return self.with_context(force_company=company.id)
