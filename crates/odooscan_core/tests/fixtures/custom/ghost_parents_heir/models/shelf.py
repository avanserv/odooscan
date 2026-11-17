from odoo import fields, models


class ShelfNote(models.AbstractModel):
    _inherit = "parent.shelf"

    note = fields.Char(compute="_compute_note")
