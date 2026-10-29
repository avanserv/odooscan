from odoo import fields, models


class ParcelBoxNote(models.Model):
    _inherit = "parcel.box"

    x_note = fields.Char(compute="_x_compute_note")  # odooscan: ignore[OD205]

    def _x_compute_note(self):
        for box in self:
            box.x_note = box.name
