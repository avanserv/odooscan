from odoo import fields, models

BIN_TAGS = "x_bin_tag_rel"


class Bin(models.Model):
    _name = "x.bin"
    _description = "Bin"

    rack_id = fields.Many2one("x.rack")
    # A related Many2many is not stored, so it keeps no table whatever BIN_TAGS holds.
    group_ids = fields.Many2many("res.groups", relation=BIN_TAGS, related="rack_id.group_ids")
    # Nor does a literal relation give a non-stored field a table.
    shelf_group_ids = fields.Many2many(
        "res.groups", relation="x_bin_shelf_rel", related="rack_id.group_ids"
    )
    # A computed field is not stored either, so Odoo names no table after its comodel.
    country_ids = fields.Many2many("res.country", compute="_compute_country_ids")
    kept_group_ids = fields.Many2many(
        "res.groups", relation="x_bin_kept_rel", related="rack_id.group_ids", store=True
    )

    def _compute_country_ids(self):
        for record in self:
            record.country_ids = False


# The unread table of m2m_rows_related is its own: nothing creates this one.
class BinTag(models.Model):
    _name = "x.bin.tag"
    _description = "Bin tag link"
    _table = "x_bin_tag_rel"
    _auto = False

    x_bin_id = fields.Many2one("x.bin", readonly=True)
    res_groups_id = fields.Many2one("res.groups", readonly=True)


class BinShelf(models.Model):
    _name = "x.bin.shelf"
    _description = "Bin shelf link"
    _table = "x_bin_shelf_rel"
    _auto = False

    x_bin_id = fields.Many2one("x.bin", readonly=True)
    res_groups_id = fields.Many2one("res.groups", readonly=True)


class BinCountry(models.Model):
    _name = "x.bin.country"
    _description = "Bin country link"
    _table = "res_country_x_bin_rel"
    _auto = False

    x_bin_id = fields.Many2one("x.bin", readonly=True)
    res_country_id = fields.Many2one("res.country", readonly=True)


# The stored related field keeps its rows here.
class BinKept(models.Model):
    _name = "x.bin.kept"
    _description = "Bin kept link"
    _table = "x_bin_kept_rel"
    _auto = False

    x_bin_id = fields.Many2one("x.bin", readonly=True)
    res_groups_id = fields.Many2one("res.groups", readonly=True)
