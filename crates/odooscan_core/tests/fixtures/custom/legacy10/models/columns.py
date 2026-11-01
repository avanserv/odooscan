from odoo import fields, models


# 10.0 makes no field of a `_columns` key, unlike 9.0.
class Legacy10Columns(models.Model):
    _name = "legacy10.columns"

    _columns = {"code": fields.Char("Code")}


class Legacy10Node(models.Model):
    _name = "legacy10.node"

    kind = fields.Char()


class Legacy10Arrow(models.Model):
    _name = "legacy10.arrow"

    signal = fields.Char()


class Legacy10ColumnsUser(models.Model):
    _name = "legacy10.columns.user"

    holder_id = fields.Many2one("legacy10.columns")
    code = fields.Char(related="holder_id.code")
