from odoo import fields, models


class StandBehind(models.AbstractModel):
    _inherit = "late.stand"

    width = fields.Integer()
