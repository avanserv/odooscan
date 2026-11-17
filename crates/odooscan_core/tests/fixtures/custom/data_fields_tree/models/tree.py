from odoo import fields, models


class DataTreeMixin(models.AbstractModel):
    _name = "data.tree.mixin"
    _description = "Tree mixin"
    _parent_store = True

    parent_path = fields.Char(index=True)


# A parent_path column without _parent_store, which data_fields_twig turns on.
class DataTreeLeaf(models.Model):
    _name = "data.tree.leaf"
    _description = "Tree leaf"

    parent_path = fields.Char(index=True)


# The mixin gives this table its parent_path column, so _auto_init does not read the _parent_name
# data_fields_twig sets.
class DataTreeBranch(models.Model):
    _name = "data.tree.branch"
    _inherit = ["data.tree.mixin"]
    _description = "Tree branch"
