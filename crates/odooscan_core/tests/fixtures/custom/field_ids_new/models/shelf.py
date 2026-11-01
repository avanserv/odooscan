from odoo import fields, models


class FieldIdsNew(models.Model):
    _name = "field.ids.new"
    _table = "field_ids_shelf"

    name = fields.Char()
    field_id = fields.Many2one("ir.model.fields")

    def base_fields(self):
        self.env.ref("base.field_res_partner__name")
        self.env.ref("base.field_res_partner_name")
