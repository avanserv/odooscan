from odoo import fields, models

INDEX = "btree_not_null"


class GardenZone(models.Model):
    _name = "garden.zone"
    _description = "Garden zone"
    _parent_store = True

    parent_id = fields.Many2one("garden.zone", ondelete="cascade")


class GardenZoneTree(models.Model):
    _inherit = "garden.zone"
    _parent_store = True


class GardenArea(models.Model):
    _name = "garden.area"
    _description = "Garden area"
    _parent_store = True

    parent_id = fields.Many2one("garden.area", ondelete="cascade")
    parent_path = fields.Char()


class GardenTerrace(models.Model):
    _name = "garden.terrace"
    _description = "Garden terrace"
    _parent_store = True

    parent_id = fields.Many2one("garden.terrace", ondelete="cascade")
    parent_path = fields.Char(index=True)


class GardenTerraceFlat(models.Model):
    _inherit = "garden.terrace"

    parent_path = fields.Char(index=False)


class GardenRow(models.Model):
    _name = "garden.row"
    _description = "Garden row"
    _parent_store = True

    parent_id = fields.Many2one("garden.row", ondelete="cascade")
    parent_path = fields.Char(index=True)


class GardenWalk(models.Model):
    _name = "garden.walk"
    _description = "Garden walk"
    _parent_store = True

    parent_id = fields.Many2one("garden.walk", ondelete="cascade")
    parent_path = fields.Char(index="btree")


class GardenTrellis(models.Model):
    _name = "garden.trellis"
    _description = "Garden trellis"
    _parent_store = True

    parent_id = fields.Many2one("garden.trellis", ondelete="cascade")
    parent_path = fields.Char(index=INDEX)


class GardenZoneReport(models.Model):
    _name = "garden.zone.report"
    _description = "Garden zone report"
    _auto = False
    _parent_store = True

    parent_id = fields.Many2one("garden.zone.report")


class GardenHierarchy(models.AbstractModel):
    _name = "garden.hierarchy"
    _description = "Garden hierarchy"
    _parent_store = True

    parent_id = fields.Many2one("garden.hierarchy")


class GardenHedge(models.Model):
    _name = "garden.hedge"
    _description = "Garden hedge"
    _parent_store = True

    parent_id = fields.Many2one("garden.hedge", ondelete="cascade")


class GardenHedgeFlat(models.Model):
    _inherit = "garden.hedge"
    _parent_store = False
