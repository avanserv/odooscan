from odoo import models


class ResPartnerDistrictHintFallback(models.Model):
    _inherit = "res.partner"

    def _compute_district_hint(self):
        super()._compute_district_hint()
        for partner in self.filtered(lambda p: not p.country_id):
            partner.district_hint = False
