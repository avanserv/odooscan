from odoo import fields, models


class BuoyDepot(models.Model):
    _name = "buoy.depot"
    _description = "Buoy depot"

    name = fields.Char()


class BuoyBase(models.AbstractModel):
    _inherit = "base"
    _inherits = {"buoy.depot": "x_base_depot_id"}


class BuoyAnchored(models.AbstractModel):
    _name = "buoy.anchored"
    _description = "Buoy anchored"
    _inherits = {"buoy.depot": "x_anchor_id"}


class BuoyPlain(models.AbstractModel):
    _name = "buoy.plain"
    _description = "Buoy plain"


# Odoo puts base last in the bases and merges them in reverse, so the entry of base comes first and
# that of buoy.anchored wins.
class BuoyFloat(models.Model):
    _name = "buoy.float"
    _inherit = ["buoy.anchored"]
    _description = "Buoy float"

    def x_float_columns(self):
        self.env.cr.execute("SELECT f.x_anchor_id, f.x_base_depot_id FROM buoy_float f")
        return self.env.cr.fetchall()


# buoy.plain carries the entry of base and merges after buoy.anchored, so the entry of base wins
# here: an order that always put base first would be wrong. base comes through both listed models,
# so the scan leaves its order unknown and both names stay silent.
class BuoyRaft(models.Model):
    _name = "buoy.raft"
    _inherit = ["buoy.plain", "buoy.anchored"]
    _description = "Buoy raft"

    def x_raft_columns(self):
        self.env.cr.execute("SELECT r.x_anchor_id, r.x_base_depot_id FROM buoy_raft r")
        return self.env.cr.fetchall()
