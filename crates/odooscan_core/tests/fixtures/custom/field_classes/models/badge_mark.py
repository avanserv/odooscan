from odoo import models
from odoo.addons.field_classes.orm.vector import Pair


# Pair is a Char or a Many2many, which has no column, so whether pair is NOT NULL is unknown.
class BadgeMark(models.Model):
    _inherit = "x.badge"

    pair = Pair(required=True)
