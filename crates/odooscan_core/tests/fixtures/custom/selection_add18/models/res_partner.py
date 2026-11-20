from odoo import fields, models


class ResPartner(models.Model):
    _inherit = "res.partner"

    sa18_stage = fields.Selection([("draft", "Draft")], required=True, default="draft")
    sa18_phase = fields.Selection([("draft", "Draft")], required=True, default="draft")
    sa18_level = fields.Selection([("draft", "Draft")], required=True, default="draft")
    sa18_grade = fields.Selection([("draft", "Draft")], required=True, default="draft")
    sa18_tier = fields.Selection(selection="_sa18_tiers", required=True, default="low")

    def _sa18_tiers(self):
        return [("low", "Low")]


class ResPartnerHeld(models.Model):
    _inherit = "res.partner"

    sa18_stage = fields.Selection(selection_add=(("held", "Held"),))
    sa18_phase = fields.Selection(selection_add=(("held", "Held"),), ondelete={"held": "cascade"})
    sa18_level = fields.Selection(selection_add=[("held",)], ondelete={"held": "cascade"})
    sa18_grade = fields.Selection(selection_add=[("held",)])
    sa18_tier = fields.Selection(selection_add=(("high", "High"),), ondelete={"high": "cascade"})
