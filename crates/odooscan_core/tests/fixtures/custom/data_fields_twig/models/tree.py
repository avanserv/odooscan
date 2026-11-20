from odoo import models

PARENT_STORE = True


# data_fields_tree gave both tables their parent_path column, the leaf by a field and the branch
# through data.tree.mixin, so _auto_init fills nothing here.
class DataTreeLeaf(models.Model):
    _inherit = "data.tree.leaf"
    _parent_store = True
    _parent_name = "x_leaf_parent_id"


class DataTreeBranch(models.Model):
    _inherit = "data.tree.branch"
    _parent_name = "x_branch_parent_id"


# This module mixes data.tree.mixin in, so the table gains parent_path here even though the mixin
# comes from a dependency.
class DataTicketLine(models.Model):
    _inherit = ["data.ticket.line", "data.tree.mixin"]
    _parent_name = "x_line_parent_id"


# A _parent_store the scan cannot evaluate keeps _parent_name on the data files.
class DataTicketCopy(models.Model):
    _inherit = "data.ticket.copy"
    _parent_store = PARENT_STORE
    _parent_name = "x_copy_parent_id"


# No class of the model sets _parent_store, so _auto_init adds no parent_path column to fill.
class DataTicketBase(models.Model):
    _inherit = "data.ticket.base"
    _parent_name = "x_base_parent_id"
