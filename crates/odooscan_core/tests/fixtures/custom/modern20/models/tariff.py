from odoo import fields, models


class ZoneLayout:
    def _x_zone_label(self):
        return "zone"


class ModernTariff(models.CachedModel):  # odooscan: ignore[OD401]
    _name = "modern.tariff"
    _description = "Tariff"

    amount = fields.Monetary(currency_field="x_tariff_currency_id")

    def _x_codes(self):
        self.env.cr.execute("SELECT t.code FROM modern_tariff t")
        return self.env.cr.fetchall()


class ModernZone(models.CachedModel, ZoneLayout):  # odooscan: ignore[OD401]
    _name = "modern.zone"
    _description = "Zone"

    amount = fields.Monetary(currency_field="x_tariff_currency_id")

    def _x_codes(self):
        self.env.cr.execute("SELECT z.code FROM modern_zone z")
        return self.env.cr.fetchall()
