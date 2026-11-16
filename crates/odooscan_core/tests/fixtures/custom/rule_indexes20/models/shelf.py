from odoo import fields, models

TIER_PARENT = "up_id"


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


class ShelfCodeMixin(models.AbstractModel):
    _name = "shelf.code.mixin"
    _description = "Shelf code mixin"

    _code_index = models.Index("(code)")

    code = fields.Char()


class ShelfCrate(models.Model):
    _name = "shelf.crate"
    _description = "Shelf crate"
    _inherit = ["shelf.code.mixin"]


class ShelfRack(models.Model):
    _name = "shelf.rack"
    _description = "Shelf rack"
    _parent_store = True

    parent_id = fields.Many2one("shelf.rack")
    parent_path = fields.Char(index=True)
    twin_id = fields.Many2one("shelf.rack")


class ShelfBay(models.Model):
    _name = "shelf.bay"
    _description = "Shelf bay"
    _parent_name = "up_id"
    _parent_store = True

    up_id = fields.Many2one("shelf.bay")
    parent_id = fields.Many2one("shelf.bay")
    parent_path = fields.Char(index=True)


class ShelfBox(models.Model):
    _name = "shelf.box"
    _description = "Shelf box"
    _inherit = ["shelf.code.mixin"]

    _code_index = models.Index("(name)")

    name = fields.Char()
    company_id = fields.Many2one("res.company")


class ShelfTier(models.Model):
    _name = "shelf.tier"
    _description = "Shelf tier"
    _parent_name = TIER_PARENT
    _parent_store = True

    up_id = fields.Many2one("shelf.tier")
    side_id = fields.Many2one("shelf.tier")
    parent_path = fields.Char(index=True)


class ShelfTreeMixin(models.AbstractModel):
    _name = "shelf.tree.mixin"
    _description = "Shelf tree mixin"
    _parent_name = "up_id"
    _parent_store = True


class ShelfLane(models.Model):
    _name = "shelf.lane"
    _description = "Shelf lane"
    _inherit = ["shelf.tree.mixin"]

    up_id = fields.Many2one("shelf.lane")
    parent_id = fields.Many2one("shelf.lane")
    parent_path = fields.Char(index=True)


class ShelfStack(models.Model):
    _name = "shelf.stack"
    _description = "Shelf stack"
    _inherit = ["shelf.tree.mixin"]
    _parent_name = "parent_id"

    up_id = fields.Many2one("shelf.stack")
    parent_id = fields.Many2one("shelf.stack")
    parent_path = fields.Char(index=True)


class ShelfPile(models.Model):
    _name = "shelf.pile"
    _description = "Shelf pile"
    _inherit = ["shelf.tree.mixin"]
    _parent_store = False

    up_id = fields.Many2one("shelf.pile")
