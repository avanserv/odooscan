from odoo import fields, models


# A version that names no series may be any series, so both rules report here.
class DriftOrchard(models.Model):  # odooscan: ignore[OD401]
    _name = "drift.orchard"
    _description = "Drift orchard"
    _parent_store = True

    name = fields.Char(string="Label")
    title = fields.Char(string="Label")


# `parent_left` belongs to series below saas~11.3, so it hints at the series the version left open.
class DriftGrove(models.Model):  # odooscan: ignore[OD401]
    _name = "drift.grove"
    _description = "Drift grove"
    _parent_store = True

    parent_left = fields.Integer(index=True)


class DriftCopse(models.Model):  # odooscan: ignore[OD401]
    _name = "drift.copse"
    _description = "Drift copse"
    _parent_store = True
    _columns = {"parent_left": None}


class DriftThicket(models.Model):  # odooscan: ignore[OD401]
    _name = "drift.thicket"
    _description = "Drift thicket"
    _parent_store = True


class DriftThicketHeld(models.Model):
    _inherit = "drift.thicket"

    parent_left = fields.Integer(index=True)
