from odoo import fields, models
from odoo.addons.field_classes14.orm.tag import Tag


class BadgeOld(models.Model):
    _name = "x.badge.old"
    _description = "Old badge"

    label = Tag(required=True)
    code = fields.Char(required=True)


# Before saas~14.4, as on 14.0, the registry walks back from the last definition of a field and
# stops at the first whose class neither is the last one's nor derives from it: label keeps
# required=True, as Tag derives from Char, and code loses it.
class BadgeOldExtension(models.Model):
    _inherit = "x.badge.old"

    label = fields.Char(string="L")
    code = Tag(string="C")
