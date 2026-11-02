from odoo import fields, models


# 11.0 keeps a hierarchy in `parent_left` and logs nothing on a repeated label.
class Legacy11Stem(models.Model):
    _name = "legacy11.stem"
    _parent_store = True

    name = fields.Char(string="Label")
    title = fields.Char(string="Label")
