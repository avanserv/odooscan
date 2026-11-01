from openerp import fields, models
from openerp.osv import fields as columns
from openerp.osv import orm

HIDDEN = {"code": columns.char("Code")}


# 9.0 makes a field of each `_columns` key, whatever column it holds.
class Legacy9Columns(orm.Model):
    _name = "legacy9.columns"

    _columns = {
        "code": columns.char("Code"),
        "parent_id": columns.many2one("legacy9.columns", "Parent"),
    }


class Legacy9ColumnsHidden(orm.Model):
    _name = "legacy9.columns.hidden"

    _columns = HIDDEN


class Legacy9ColumnsUser(models.Model):
    _name = "legacy9.columns.user"

    holder_id = fields.Many2one("legacy9.columns", domain=[("code", "=", "a")])
    hidden_id = fields.Many2one("legacy9.columns.hidden", domain=[("ghost", "=", "a")])
    code = fields.Char(related="holder_id.code")
    parent_ghost = fields.Char(related="holder_id.parent_id.ghost")
    ghost = fields.Char(related="holder_id.ghost")
