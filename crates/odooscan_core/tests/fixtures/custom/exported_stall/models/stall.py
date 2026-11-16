from odoo import _, _lt, models

RENT = _lt("Stall rent is due")


class ExportedStall(models.AbstractModel):
    _name = "exported_stall.stall"
    _description = "Stall"

    def _(self, message):
        return message

    def stall_messages(self, name, count):
        tidied = {"name": _("Stall tidied"), "rows": [("Stall shelf", count)]}
        return [
            _("Stall opened"),
            _("Stall keeper"),
            _("Stall closed"),
            self.env._("Stall rented"),
            _("Stall moved"),
            _("Stall moved"),
            _("Stall swept"),
            _("Stall painted"),
            _(("Stall lit")),
            _(f"Stall dusted"),
            _("Stall %s", _("Stall nested")),
            _("Stall " + "corner"),
            _("Stall of %s" % name),  # odooscan: ignore[OD102]
            tidied,
            _("%s"),
            _("12"),
            _("Stall ignored"),  # odooscan: ignore[OD804]
        ]
