from odoo import fields, models


class LegacyStamp(models.Model):
    _name = "legacy.stamp"
    _description = "Legacy stamp"

    name = fields.Char()


class LegacyLedger(models.Model):
    _name = "legacy.ledger"
    _description = "Legacy ledger"
    _inherit = "legacy.stamp"

    create_date = fields.Datetime(index=True)
    opened_on = fields.Datetime(string="Created on")
    stamped_on = fields.Datetime(string="Create Date")
