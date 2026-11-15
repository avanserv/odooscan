from odoo import fields, models


class ShelfBin(models.Model):
    _name = "shelf.bin"
    _description = "Shelf bin"

    _aisle_index = models.Index("(aisle)")

    name = fields.Char()
    zone = fields.Char()
    row = fields.Integer()
    aisle = fields.Char()
    level = fields.Integer()
    area_id = fields.Many2one("shelf.zone", bypass_search_access=True)


class ShelfZone(models.Model):
    _name = "shelf.zone"
    _description = "Shelf zone"

    name = fields.Char()
