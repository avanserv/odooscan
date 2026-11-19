from odoo import fields, models

BIN_TAGS = "x_bin_tag_rel"


class Bin(models.Model):
    _name = "x.bin"
    _description = "Bin"

    rack_id = fields.Many2one("x.rack")
    # A related Many2many is not stored, so it keeps no table whatever BIN_TAGS holds.
    group_ids = fields.Many2many("res.groups", relation=BIN_TAGS, related="rack_id.group_ids")


# The unread table of m2m_rows_related is its own: nothing creates this one.
class BinTag(models.Model):
    _name = "x.bin.tag"
    _description = "Bin tag link"
    _table = "x_bin_tag_rel"
    _auto = False

    x_bin_id = fields.Many2one("x.bin", readonly=True)
    res_groups_id = fields.Many2one("res.groups", readonly=True)
