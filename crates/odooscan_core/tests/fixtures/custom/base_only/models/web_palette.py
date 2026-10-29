from odoo import fields, models


class WebPalette(models.Model):  # odooscan: ignore[OD401]
    _name = "web.palette"

    accent = fields.Char()
