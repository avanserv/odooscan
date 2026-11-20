from odoo import fields, models


# The table of data.ticket gains parent_path here, so _auto_init fills it through _parent_name
# before data/fields.xml records that field.
class DataTicket(models.Model):
    _inherit = "data.ticket"
    _parent_store = True
    _parent_name = "x_tree_parent_id"

    parent_path = fields.Char(index=True)
