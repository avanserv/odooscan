from odoo import fields, models


class FieldIdsLater(models.Model):
    _name = "field.ids.later"

    name = fields.Char()
    field_id = fields.Many2one("ir.model.fields")
