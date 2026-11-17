from odoo import models


class GhostLampShade(models.Model):
    _inherit = "ghost.lamp"


class SiblingLampShade(models.AbstractModel):
    _inherit = "sibling.lamp"
