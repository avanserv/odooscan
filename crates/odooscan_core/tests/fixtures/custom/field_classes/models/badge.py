from odoo import fields, models
from odoo.addons.field_classes.orm.vector import Tag


class Badge(models.Model):
    _name = "x.badge"
    _description = "Badge"

    label = Tag(required=True)
    code = fields.Char(required=True)


# From saas~14.4 to 20.0 the registry builds a field as the class of its last definition, and drops
# what an earlier definition sets unless that class is the earlier one's or derives from it: label
# loses required=True, as Char does not derive from Tag, and code keeps it.
class BadgeExtension(models.Model):
    _inherit = "x.badge"

    label = fields.Char(string="L")
    code = Tag(string="C")
