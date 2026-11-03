from odoo import api, models


class ResPartnerUpgrade(models.Model):
    _inherit = "res.partner"

    def _x_totals(self):
        return self.env["res.partner"].read_group([], fields=["name"], groupby=["country_id"])

    @api.model
    def read_group(self, domain, fields, groupby, offset=0, limit=None, orderby=False, lazy=True):
        return super().read_group(domain, fields, groupby, offset=offset, limit=limit, orderby=orderby, lazy=lazy)
