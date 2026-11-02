from odoo import fields, models


class UnparsedLedger(models.Model):
    _inherit = "unparsed.ledger"

    note = fields.Char()
    state = fields.Selection(selection_add=[("late", "Late")])


class UnparsedFresh(models.Model):
    _name = "unparsed.fresh"

    name = fields.Char()


class UnparsedReport(models.Model):
    _inherit = "unparsed.report"


class ResCurrency(models.Model):
    _inherit = "res.currency"

    def action_close_books(self):
        print "closing"

    def _close_books(self):
        print "closing"
