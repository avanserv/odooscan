from odoo import fields, models


class SaasBeforeLedger(models.Model):
    _name = "saas.before.ledger"
    _description = "SaaS before ledger"

    name = fields.Char()
