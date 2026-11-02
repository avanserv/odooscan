from odoo import fields, models


class RackAwayNew(models.Model):
    _inherit = "ghost.rack"

    stock = fields.Integer()

    def stock_fields(self):
        self.env.ref("field_ids_away12.field_ghost_rack__stock")
        self.env.ref("field_ids_away12.field_rack_table_stock")
