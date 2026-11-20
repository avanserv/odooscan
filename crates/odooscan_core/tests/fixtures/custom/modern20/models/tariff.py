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


class ModernTariffCache(models.Model):
    _inherit = "modern.tariff"

    _clear_cache_name = "stable"

    def _cached_data(self):
        return super()._cached_data()

    def get_all(self):
        return super().get_all()

    def _x_cached_domain(self):
        return super()._cached_data_domain()


class ModernZoneCache(models.Model):
    _inherit = "modern.zone"

    _cached_data = fields.Char()


class ModernRate(models.CachedModel):  # odooscan: ignore[OD401]
    _name = "modern.rate"
    _description = "Rate"

    get_all = fields.Char()
    _cached_data_fields = fields.Char()


class ModernPlainRate(models.Model):  # odooscan: ignore[OD401]
    _name = "modern.plain.rate"
    _description = "Plain rate"

    get_all = fields.Char()
    _cached_data_fields = fields.Char()
