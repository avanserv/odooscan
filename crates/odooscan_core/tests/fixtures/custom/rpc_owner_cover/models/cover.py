from odoo import fields, models


class LampCover(models.Model):
    _inherit = "rpc.owner.lamp"

    cover_colour = fields.Char()


class MastCover(models.Model):
    _inherit = "rpc.owner.mast"

    cover_colour = fields.Char()
