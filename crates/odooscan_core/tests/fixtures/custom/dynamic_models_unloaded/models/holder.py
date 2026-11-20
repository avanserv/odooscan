from odoo import fields, models


class UnloadedHolder(models.Model):
    _name = "unloaded.holder"

    ghost_id = fields.Many2one("ghost.unloaded")
