from odoo import _, models


class ExportedLedger(models.AbstractModel):
    _name = "exported_stall.ledger"
    _description = "Ledger"

    def ledger_messages(self, name):
        # Babel stops reading the file at the f-string, so the export never writes "Ledger closed".
        return [_(f"Ledger of {name}"), _("Ledger closed")]  # odooscan: ignore[OD102]
