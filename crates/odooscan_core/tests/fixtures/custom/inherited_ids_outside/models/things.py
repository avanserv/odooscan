from odoo import fields, models
from odoo.addons.ghost_mixins.models.layout import GhostLayout


class Hosted(models.Model):
    _name = "inherited.outside.hosted"
    _inherit = "ghost.mixin"
    _description = "Hosted"

    field_id = fields.Many2one("ir.model.fields")

    def fields_named(self):
        self.env.ref("inherited_ids_outside.field_inherited_outside_hosted__tag")
        self.env.ref("inherited_ids_outside.field_inherited_outside_plain__tag")


class Plain(models.Model):
    _name = "inherited.outside.plain"
    _description = "Plain"

    name = fields.Char()


class Based(GhostLayout, models.Model):
    _name = "inherited.outside.based"
    _description = "Based"

    name = fields.Char()


class Hazed(models.Model):
    _name = "inherited.outside.hazed"
    _inherit = "inherited.outside.hazy"
    _description = "Hazed"
