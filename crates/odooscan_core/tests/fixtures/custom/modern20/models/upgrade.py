from odoo import api, models


class ResPartnerUpgrade(models.Model):
    _inherit = "res.partner"

    def _x_totals(self):
        partners = self.env["res.partner"]
        partners.read_group([], groupby=["country_id"], aggregates=["__count"])
        return partners.read_group([], fields=["name"], groupby=["country_id"], lazy=False)

    @api.model
    def read_group(self, domain, groupby=(), aggregates=(), having=(), offset=0, limit=None, order=None):
        return super().read_group(domain, groupby, aggregates, having=having, offset=offset, limit=limit, order=order)
