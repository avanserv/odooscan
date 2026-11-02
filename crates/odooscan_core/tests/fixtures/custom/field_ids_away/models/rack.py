from odoo import fields, models


class RackAway(models.Model):
    _inherit = "ghost.rack"

    stock = fields.Integer()
    shelf = fields.Char()
    name = fields.Char()

    def stock_field(self):
        self.env.ref("field_ids_away.field_rack_table_stock")
        self.env.ref("field_ids_away.field_rack_table_tally")


class OldAway(models.Model):
    _inherit = "field.ids.old"

    tally = fields.Integer()
