from odoo import fields, models


class StandAhead(models.AbstractModel):
    _inherit = "late.stand"

    height = fields.Integer()
