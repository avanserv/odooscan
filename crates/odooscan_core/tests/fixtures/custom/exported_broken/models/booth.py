from odoo import _, models


class ExportedBooth(models.AbstractModel):
    _name = "exported_broken.booth"
    _description = "Booth"

    def booth_message(self):
        return _("Booth sold")
