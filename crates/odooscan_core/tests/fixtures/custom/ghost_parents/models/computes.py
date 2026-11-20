from odoo import fields, models


class ShelfLabel(models.AbstractModel):
    _inherit = "parent.shelf"

    label = fields.Char(compute="_compute_label")


class ParentRack(models.AbstractModel):
    _name = "parent.rack"
    _description = "Parent rack"
    _inherit = ["parent.shelf"]

    label = fields.Char(compute="_compute_label")


class BenchLabel(models.AbstractModel):
    _inherit = "parent.bench"

    label = fields.Char(compute="_compute_label")
