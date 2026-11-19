from odoo import fields, models


# No indexed class introduces ghost.lantern, so its fields are not all known.
class GhostLantern(models.AbstractModel):
    _inherit = "ghost.lantern"

    wick = fields.Char()


class SideLantern(models.AbstractModel):
    _name = "side.lantern"
    _description = "Side lantern"

    wick = fields.Char()
