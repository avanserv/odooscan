from odoo import fields, models


class PlainSchedule:
    def shift(self):
        return self


class WindowShared(PlainSchedule, models.Model):
    _name = "window.shared"
    _description = "Window shared"

    name = fields.Char()


class WindowBorrower(models.Model):
    _name = "window.borrower"
    _description = "Window borrower"
    _inherits = {"window.shared": "shared_id"}

    shared_id = fields.Many2one("window.shared", required=True, ondelete="cascade")


class WindowCopied(models.Model):
    _name = "window.copied"
    _description = "Window copied"
    _inherit = ["window.shared"]


class WindowDatedParent(models.Model):
    _name = "window.dated.parent"
    _description = "Window dated parent"

    date_start = fields.Datetime()
    date_stop = fields.Datetime()


class WindowDatedChild(models.Model):
    _name = "window.dated.child"
    _description = "Window dated child"
    _inherits = {"window.dated.parent": "parent_id"}

    parent_id = fields.Many2one("window.dated.parent", required=True, ondelete="cascade")
