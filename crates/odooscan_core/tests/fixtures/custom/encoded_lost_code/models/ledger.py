from odoo import api, fields, models


class Ledger(models.Model):
    _inherit = "encoded.ledger"
    _order = "x_lost"

    balance = fields.Monetary()
    lost_balance = fields.Integer(compute="_compute_lost_balance")

    def action_settle(self):
        return super().action_settle()

    def _totals(self):
        self.env.cr.execute("SELECT t.no_such_column FROM encoded_ledger t")

    def _lost_lookup(self):
        return self.search([("x_lost", "=", 1)])

    @api.depends("x_lost")
    def _compute_lost_balance(self):
        self.lost_balance = 0


class Note(models.Model):
    _name = "encoded.note"
    _description = "Note"
    _order = "x_lost"

    ledger_ids = fields.Many2many("encoded.ledger")
    lost_total = fields.Integer(compute="_compute_lost_total")

    def _note_lookup(self):
        return self.search([("no_note_search", "=", 1)])

    @api.depends("x_lost")
    def _compute_lost_total(self):
        self.lost_total = 0
