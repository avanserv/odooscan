from odoo import models


class ParcelManifestView(models.Model):
    _inherit = "parcel.manifest"

    def init(self):
        self.env.cr.execute("CREATE OR REPLACE VIEW parcel_manifest AS (SELECT 1 AS id, 0 AS line_count)")


class WaybillViewBuilder:
    def init(self):
        self.env.cr.execute(self._x_waybill_query())


class ParcelWaybillView(models.Model, WaybillViewBuilder):
    _inherit = "parcel.waybill"

    def _x_waybill_query(self):
        return "SELECT 1"


class ParcelDrawer(models.Model):
    _name = "parcel.drawer"
    _description = "Parcel drawer"
    _auto = False


QUOTA_BUILDER = "parcel.depot"


class ParcelQuotaView(models.Model):
    _name = "parcel.quota"
    _inherit = ["parcel.quota", QUOTA_BUILDER]
