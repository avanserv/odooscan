from odoo import fields, models


class UnparsedLedger(models.Model):
    _name = "unparsed.ledger"

    name = fields.Char()
    state = fields.Selection([("open", "Open")])
    partner_id = fields.Many2one("res.partner")


class UnparsedReport(models.Model):
    _name = "unparsed.report"
    _auto = False

    name = fields.Char()
