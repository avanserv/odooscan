from odoo import fields, models


class DataTreeNode(models.Model):
    _name = "data.tree.node"
    _description = "Tree node"

    name = fields.Char()


# From saas~13.3 the module creates this table with parent_path among its columns, so nothing is
# filled ahead of data/fields.xml.
class DataTreeNodeStore(models.Model):
    _inherit = "data.tree.node"
    _parent_store = True
    _parent_name = "x_node_parent_id"

    parent_path = fields.Char(index=True)
