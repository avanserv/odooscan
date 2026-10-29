from odoo import fields, models


class FreightPallet(models.Model):
    _name = "freight.pallet"
    _description = "Freight pallet"
    _sql_constraints = [("bogus_positive", "CHECK(bogus > 0)", "Odoo ignores this list from 19.0.")]

    name = fields.Char()
    lane = fields.Char()

    _lane_required = models.Constraint("CHECK(lane_code IS NOT NULL)", "A pallet needs a lane.")
    _name_slot_index = models.Index("(name, slot)")
    _lane_name_unique = models.UniqueIndex("(lane, name) WHERE bin IS NOT NULL", "A lane holds a name once.")
    _dock_index = models.Index(lambda registry: "(bogus_dock)")
