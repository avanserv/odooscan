from odoo import fields, models


class ResPartner(models.Model):
    _inherit = "res.partner"

    rank_old_note = fields.Char()
