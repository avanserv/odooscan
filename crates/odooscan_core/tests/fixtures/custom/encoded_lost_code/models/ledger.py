from odoo import fields, models


class Ledger(models.Model):
    _inherit = "encoded.ledger"

    balance = fields.Monetary()

    def action_settle(self):
        return super().action_settle()

    def _totals(self):
        self.env.cr.execute("SELECT t.no_such_column FROM encoded_ledger t")

    def _lost_lookup(self):
        return self.search([("x_lost", "=", 1)])


class Note(models.Model):
    _name = "encoded.note"
    _description = "Note"

    def _note_lookup(self):
        return self.search([("no_note_search", "=", 1)])
