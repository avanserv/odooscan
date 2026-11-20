from odoo import fields, models
from odoo.addons.field_classes_any.orm.tag import Tag


class BadgeAny(models.Model):
    _name = "x.badge.any"
    _description = "Badge of any series"

    label = Tag(required=True)
    code = fields.Char(required=True)


# Before saas~14.4 label keeps required=True and code loses it; from saas~14.4 to 20.0 the other
# way round. With no series in the version, neither field is known to be required.
class BadgeAnyExtension(models.Model):
    _inherit = "x.badge.any"

    label = fields.Char(string="L")
    code = Tag(string="C")
