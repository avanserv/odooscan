from odoo import fields, models


# Before saas~13.3 Odoo creates the table with id alone, so _auto_init adds parent_path and fills
# it through _parent_name before data/fields.xml records that field.
class ParentStoreNode(models.Model):
    _name = "parent.store12.node"
    _description = "Node"
    _parent_store = True
    _parent_name = "x_node_parent_id"

    parent_path = fields.Char(index=True)
