from odoo import fields, models


class SaasAfterLedger(models.Model):
    _name = "saas.after.ledger"
    _description = "SaaS after ledger"

    name = fields.Char()
