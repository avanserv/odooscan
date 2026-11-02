from odoo import api, fields, models


# The shape of Odoo core: product.product delegates to product.template, whose barcode is related
# to product_variant_ids.barcode, and its own barcode shadows that one, so no cycle exists.
class Crate(models.Model):
    _name = "shadow.crate"

    box_ids = fields.One2many("shadow.box", "crate_id")
    label = fields.Char(related="box_ids.label")
    size = fields.Many2one("res.partner")


class Box(models.Model):
    _name = "shadow.box"
    _inherits = {"shadow.crate": "crate_id"}

    crate_id = fields.Many2one("shadow.crate", required=True, ondelete="cascade")
    label = fields.Char()
    size = fields.Char()
    shown = fields.Char(compute="_compute_shown")

    @api.depends("label", "size.name")
    def _compute_shown(self):
        self.shown = False
