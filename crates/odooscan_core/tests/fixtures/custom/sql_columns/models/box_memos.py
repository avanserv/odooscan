from odoo import fields, models


class ParcelBoxMemo(models.Model):
    _inherit = "parcel.box"

    x_note = fields.Text()  # odooscan: ignore[OD205]
