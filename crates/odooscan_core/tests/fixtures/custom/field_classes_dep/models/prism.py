from odoo import fields, models
from odoo.addons.field_classes.orm.vector import Vector
from odoo.addons.field_classes_dep.orm.mark import Mark


class Prism(models.Model):
    _name = "x.prism"
    _description = "Prism"

    embedding = Vector(size=3)
    lens_mark = Mark("x.lens")

    def _known_fields(self):
        return self.search([("embedding", "=", False)])


class Lens(models.Model):
    _inherit = "x.lens"

    mark = fields.Char()
    prism_ids = fields.One2many("x.prism", "lens_mark")
