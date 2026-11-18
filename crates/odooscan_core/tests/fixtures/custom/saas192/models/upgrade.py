from odoo import api, models
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

    @api.model
    def read_group(self, domain, fields, groupby, offset=0, limit=None, orderby=False, lazy=True):
        return super().read_group(domain, fields, groupby, offset=offset, limit=limit, orderby=orderby, lazy=lazy)


try:
    from odoo.fields import Domain
except ImportError:
    from odoo.osv import expression as domain_expression

try:
    from odoo.tools import float_round
except ImportError:
    from odoo.osv import expression as round_expression
