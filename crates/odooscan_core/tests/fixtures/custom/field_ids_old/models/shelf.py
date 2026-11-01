from odoo import fields, models


class FieldIdsOld(models.Model):
    _name = "field.ids.old"
    _table = "field_ids_stock"

    name = fields.Char()
    code = fields.Char(required=True)
    field_id = fields.Many2one("ir.model.fields")


class FieldIdsPlain(models.Model):
    _name = "field.ids.plain"

    name = fields.Char()
