from odoo import _, models


def _lt(message):
    return message


RENT = _lt("Stall rent is due")


class ExportedStall(models.AbstractModel):
    _name = "exported_stall12.stall"
    _description = "Stall"

    def stall_message(self):
        return _("Stall opened")
