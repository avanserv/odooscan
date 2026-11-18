from odoo import fields, models
from odoo.osv import expression


class ResPartnerUpgrade(models.Model):
    _inherit = "res.partner"

    x_legacy_code = fields.Char(oldname="x_code")

    def _x_domain(self):
        return expression.AND([[("name", "=", "x")], [("country_id", "=", False)]])

    def _x_reset_lang(self):
        return self.with_context(lang=self._context.get("lang"), reset_by=self._uid)


class ModernCursorHolder:
    def __init__(self, cursor):
        self._cr = cursor

    def cursor(self):
        return self._cr


try:
    from odoo.fields import Domain
except ImportError:
    from odoo.osv import expression as domain_expression

try:
    from odoo.tools import float_round
except ImportError:
    from odoo.osv import expression as round_expression
