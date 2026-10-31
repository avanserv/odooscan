from odoo import fields, models


class LegacySlip(models.TransientModel):
    _name = "legacy.slip"
    _description = "Legacy slip"

    name = fields.Char()
