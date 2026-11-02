from odoo import models


class UnparsedReader(models.Model):
    _inherit = "res.partner"

    def _built_records(self):
        return self.env["unparsed.built.first"].search([])
