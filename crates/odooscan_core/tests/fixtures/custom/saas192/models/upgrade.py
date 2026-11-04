from odoo import models
from odoo.osv import expression, query

try:
    from odoo.osv.expression import normalize_domain
except ImportError:
    normalize_domain = None


class ResPartnerUpgrade(models.Model):
    _inherit = "res.partner"

    def _x_check(self):
        self.check_access_rights("read")
        self.check_access_rule("read")
        self.env["res.partner"].read_group([], fields=["name"], groupby=["country_id"])
        return expression, query, normalize_domain
