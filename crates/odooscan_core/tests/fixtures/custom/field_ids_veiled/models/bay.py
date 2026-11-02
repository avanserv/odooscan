from odoo import fields, models


class BayVeiled(models.Model):
    _inherit = "ghost.bay"
    _inherits = {"ghost.frame": "frame_id"}

    frame_id = fields.Many2one("ghost.frame")
