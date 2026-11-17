from odoo import fields, models


# Before 12.0 _auto_init leaves a new parent_path column empty, so _parent_name resolves against
# data/fields.xml.
class ParentStoreNode(models.Model):
    _name = "parent.store115.node"
    _description = "Node"
    _parent_store = True
    _parent_name = "x_node_parent_id"

    parent_path = fields.Char(index=True)
