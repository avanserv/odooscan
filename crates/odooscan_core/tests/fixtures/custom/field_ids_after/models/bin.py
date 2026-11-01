from odoo import fields, models


class FieldIdsBroken(models.Model):
    _inherit = "field.ids.broken"

    extra = fields.Char()


class FieldIdsAfter(models.Model):
    _name = "field.ids.after"

    field_id = fields.Many2one("ir.model.fields")

    def fields_named(self):
        self.env.ref("field_ids_broken.field_field_ids_bin_name")
        self.env.ref("field_ids_after.field_field_ids_bin_extra")
        self.env.ref("field_ids_broken.field_field_ids_gone_name")
