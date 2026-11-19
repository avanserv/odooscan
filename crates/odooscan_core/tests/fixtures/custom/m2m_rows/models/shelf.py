from odoo import fields, models

SHELF_TAGS = "x_shelf_tag_rel"


class Shelf(models.Model):
    _name = "x.shelf"
    _description = "Shelf"

    # The scan cannot name the table SHELF_TAGS holds, so it may be the one x.shelf.tag reads.
    tag_ids = fields.Many2many("res.country", relation=SHELF_TAGS)


class ShelfTag(models.Model):
    _name = "x.shelf.tag"
    _description = "Shelf tag link"
    _table = "x_shelf_tag_rel"
    _auto = False

    x_shelf_id = fields.Many2one("x.shelf", readonly=True)
    res_country_id = fields.Many2one("res.country", readonly=True)
