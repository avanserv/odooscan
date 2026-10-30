from odoo import api, fields, models

MONEY = {"currency_field": "ledger_currency_id"}


class RegistryLedger(models.Model):
    _name = "registry.ledger"
    _description = "Ledger"

    currency_id = fields.Many2one("res.currency")
    balance = fields.Monetary()


class RegistryCustom(models.Model):
    _name = "registry.custom"
    _description = "Studio-like model"

    x_currency_id = fields.Many2one("res.currency")
    x_amount = fields.Monetary()


class RegistryInvoice(models.Model):
    _name = "registry.invoice"
    _description = "Invoice"

    partner_id = fields.Many2one("res.partner")
    ledger_id = fields.Many2one("registry.ledger")
    paid_currency_id = fields.Many2one("res.currency")
    amount = fields.Monetary()
    tax = fields.Monetary(currency_field="tax_currency_id")
    ledger_balance = fields.Monetary(related="ledger_id.balance")
    paid = fields.Monetary("Paid", "paid_currency_id")
    carried = fields.Monetary(**MONEY)
    partner_ref = fields.Char(compute="_compute_partner_ref")
    partner_key = fields.Char(compute="_compute_partner_key")

    @api.depends("id")
    def _compute_partner_ref(self):
        for invoice in self:
            invoice.partner_ref = str(invoice.id)

    @api.depends("partner_id.id")
    def _compute_partner_key(self):
        for invoice in self:
            invoice.partner_key = str(invoice.partner_id.id)
