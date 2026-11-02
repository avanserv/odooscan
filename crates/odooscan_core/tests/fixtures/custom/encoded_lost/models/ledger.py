from odoo import api, fields, models


class Ledger(models.Model):
    _name = "encoded.ledger"
    _description = "Ledger"
    _order = "x_lost"

    code = fields.Char(required=True)
    total = fields.Integer(compute="_compute_total")
    stage = fields.Selection([("open", "Open"), ("closed", "Closed")])

    def _legacy_group(self):
        return self.env.ref("encoded_lost.legacy_ledger")

    def _legacy_lookup(self):
        return self.search([("x_lost", "=", 1)])

    @api.depends("x_lost")
    def _compute_total(self):
        self.total = 0


class Partner(models.Model):
    _inherit = "res.partner"

    ledger_total = fields.Integer(compute="_compute_ledger_total")

    @api.depends("x_lost")
    def _compute_ledger_total(self):
        self.ledger_total = 0
