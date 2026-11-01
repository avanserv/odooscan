from odoo import fields, models


class UnparsedFresh(models.Model):
    _inherit = "unparsed.fresh"

    extra = fields.Char()
