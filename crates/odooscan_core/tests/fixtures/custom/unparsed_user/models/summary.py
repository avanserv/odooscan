from odoo import fields, models


class UnparsedSummary(models.Model):
    _name = "unparsed.summary"

    ledger_id = fields.Many2one("unparsed.ledger")
    fresh_id = fields.Many2one("unparsed.fresh")
    note = fields.Char(related="ledger_id.note")
    fresh_name = fields.Char(related="fresh_id.label")
    fresh_extra = fields.Char(related="fresh_id.extra")


class UnparsedCopy(models.Model):
    _name = "unparsed.copy"
    _inherit = "unparsed.fresh"


class UnparsedWrap(models.Model):
    _name = "unparsed.wrap"
    _inherits = {"unparsed.fresh": "fresh_id"}

    fresh_id = fields.Many2one("unparsed.fresh", required=True, ondelete="cascade")


class UnparsedLedger(models.Model):
    _inherit = "unparsed.ledger"

    def action_reconcile(self):
        return super(UnparsedLedger, self).action_reconcile()
