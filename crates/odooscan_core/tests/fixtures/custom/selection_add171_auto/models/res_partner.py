from odoo import fields, models


class ResPartner(models.Model):
    _inherit = "res.partner"

    sa171_auto_stage = fields.Selection([("draft", "Draft")], required=True, default="draft")


class ResPartnerHeld(models.Model):
    _inherit = "res.partner"

    sa171_auto_stage = fields.Selection(selection_add=[("held",)], ondelete={"held": "cascade"})
