from odoo import fields, models


class FieldIdsBroken(models.Model):
    _name = "field.ids.broken"
    _table = "field_ids_bin"

    name = fields.Char()

    def report(self):
        print "x"
