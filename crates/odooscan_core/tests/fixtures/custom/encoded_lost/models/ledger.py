from odoo import fields, models


class Ledger(models.Model):
    _name = "encoded.ledger"
    _description = "Ledger"

    code = fields.Char(required=True)

    def _legacy_group(self):
        return self.env.ref("encoded_lost.legacy_ledger")

    def _legacy_lookup(self):
        return self.search([("x_lost", "=", 1)])
