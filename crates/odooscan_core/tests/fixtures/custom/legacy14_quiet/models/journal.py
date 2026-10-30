from odoo import models


class LegacyJournalQuiet(models.Model):
    _inherit = "legacy.journal"
    _log_access = False

    def x_journal_stamps(self):
        self.env.cr.execute("SELECT j.name, j.create_date FROM legacy_journal j")
        return self.env.cr.fetchall()
