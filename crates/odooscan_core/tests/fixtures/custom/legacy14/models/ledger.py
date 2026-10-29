from odoo import fields, models


class LegacyLedger(models.Model):  # odooscan: ignore[OD401]
    _name = "legacy.ledger"
    _description = "Legacy ledger"

    name = fields.Char()


class LegacyLedgerQuiet(models.Model):
    _inherit = "legacy.ledger"
    _log_access = False

    def x_ledger_stamps(self):
        self.env.cr.execute("SELECT l.name, l.create_date, l.__last_update FROM legacy_ledger l")
        return self.env.cr.fetchall()


class LegacyJournal(models.Model):  # odooscan: ignore[OD401]
    _name = "legacy.journal"
    _description = "Legacy journal"

    name = fields.Char()
