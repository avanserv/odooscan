from odoo import models
from odoo.addons.field_classes.orm.vector import Tag


# On 20.0 seal is built as the class of whichever of this file and seal_char.py loads last, which
# the imports decide: as a Tag it keeps required=True, as a Char it drops it.
class BadgeSealTag(models.Model):
    _inherit = "x.badge"

    seal = Tag(required=True)
