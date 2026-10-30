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


class LegacyTally(models.Model):
    _name = "legacy.tally"
    _description = "Legacy tally"

    create_date = fields.Datetime(copy=True)


class LegacySlip(models.Model):
    _name = "legacy.slip"
    _description = "Legacy slip"
    _inherit = "legacy.tally"

    create_date = fields.Datetime(copy=False)
    filed_on = fields.Datetime(string="Created on")
