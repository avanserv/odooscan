from odoo import models


class GhostWidget(models.AbstractModel):
    _inherit = "ghost.widget"


class ParentShelf(models.AbstractModel):
    _name = "parent.shelf"
    _description = "Parent shelf"
    _inherit = ["ghost.tray"]


class GhostTray(models.AbstractModel):
    _inherit = "ghost.tray"


class ParentSeat(models.AbstractModel):
    _name = "parent.seat"
    _description = "Parent seat"


class ParentBench(models.AbstractModel):
    _name = "parent.bench"
    _description = "Parent bench"
    _inherit = ["parent.seat"]
