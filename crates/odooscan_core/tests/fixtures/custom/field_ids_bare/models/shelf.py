from odoo import fields, models


class FieldIdsBare(models.Model):
    _name = "field.ids.bare"

    name = fields.Char()
    field_id = fields.Many2one("ir.model.fields")
