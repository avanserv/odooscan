import openerp
from openerp import fields as fields2
from openerp import models


class ResPartner(models.Model):
    _inherit = "res.partner"

    legacy_code = openerp.fields.Char()
    legacy_label = fields2.Char()

    def _legacy_fields(self):
        return self.search([("legacy_code", "=", "a"), ("legacy_label", "=", "b")])
