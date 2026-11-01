from odoo import fields, models


# A version that names no series may be any series, so both rules report here.
class DriftOrchard(models.Model):  # odooscan: ignore[OD401]
    _name = "drift.orchard"
    _description = "Drift orchard"
    _parent_store = True

    name = fields.Char(string="Label")
    title = fields.Char(string="Label")
