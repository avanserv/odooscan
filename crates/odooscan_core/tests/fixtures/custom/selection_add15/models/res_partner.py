from odoo import fields, models


class ResPartner(models.Model):
    _inherit = "res.partner"

    sa15_stage = fields.Selection(
        [("draft", "Draft")], selection_add=[("held", "Held")], required=True, default="draft"
    )
