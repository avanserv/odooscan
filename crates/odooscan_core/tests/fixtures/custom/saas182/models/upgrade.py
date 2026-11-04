from odoo import models


class ResPartnerUpgrade(models.Model):
    _inherit = "res.partner"

    def _x_totals(self):
        return self.env["res.partner"].read_group([], fields=["name"], groupby=["country_id"])
