from odoo import models


class UnparsedLedger(models.Model):
    _inherit = "unparsed.ledger"

    def action_settle(self):
        return super(UnparsedLedger, self).action_settle()
