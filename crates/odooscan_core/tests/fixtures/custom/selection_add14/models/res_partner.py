from odoo import fields, models


class ResPartner(models.Model):
    _inherit = "res.partner"

    sa14_stage = fields.Selection(
        [("draft", "Draft")], selection_add=[("held", "Held")], required=True, default="draft"
    )
    sa14_phase = fields.Selection(
        [("draft", "Draft")],
        selection_add=[("held", "Held")],
        ondelete={"held": "cascade"},
        required=True,
        default="draft",
    )
