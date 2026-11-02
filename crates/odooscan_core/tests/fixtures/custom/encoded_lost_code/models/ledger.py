from odoo import fields, models


class Ledger(models.Model):
    _inherit = "encoded.ledger"

    balance = fields.Monetary()

    def action_settle(self):
        return super().action_settle()

    def _totals(self):
        self.env.cr.execute("SELECT t.no_such_column FROM encoded_ledger t")
