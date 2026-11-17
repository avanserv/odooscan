from odoo import fields, models


# data_fields_tree gave the table its parent_path column, so _auto_init fills nothing here.
class DataTicket(models.Model):
    _inherit = "data.ticket"
    _parent_store = True
    _parent_name = "x_twig_parent_id"

    parent_path = fields.Char(index=True)
