from odoo import _, models


class ExportedCounter(models.AbstractModel):
    _name = "exported_stall.counter"
    _description = "Counter"

    def counter_messages(self, name):
        # Babel evaluates each f-string before Python 3.12 and stops reading the file at the one
        # it fails on, so the export writes "Counter opened" but may never write "Counter emptied".
        return [
            _("Counter opened"),
            _(f"Counter {'shut'}"),
            _(f"Counter bell" + _("Counter rope") + " rung"),  # odooscan: ignore[OD102]
            _(f"Counter of the {f'Counter tray'}"),  # odooscan: ignore[OD102]
            _(f"Counter of {name}"),  # odooscan: ignore[OD102]
            _("Counter emptied"),
        ]
