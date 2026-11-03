from odoo import models


class ResPartnerUpgrade(models.Model):
    _inherit = "res.partner"

    def _x_totals(self):
        partners = self.env["res.partner"]
        partners.read_group([], groupby=["country_id"], aggregates=["__count"])
        return partners.read_group([], fields=["name"], groupby=["country_id"], lazy=False)
