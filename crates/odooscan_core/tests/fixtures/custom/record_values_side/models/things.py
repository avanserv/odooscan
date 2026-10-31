from odoo import fields, models

KW = {"ondelete": {"z": "cascade"}}


class RvThing(models.Model):
    _inherit = "rv.thing"

    added = fields.Selection(selection_add=[("z", "Zed")])
    spread_kind = fields.Selection(selection_add=[("z", "Zed")], **KW)


class RvSideOnly(models.Model):
    _name = "rv.side.only"
    _description = "Outside the depends of record_values"

    name = fields.Char()
