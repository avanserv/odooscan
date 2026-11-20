from odoo import fields, models


class LampShade(models.Model):
    _inherit = "rpc.owner.lamp"

    shade_colour = fields.Char()
