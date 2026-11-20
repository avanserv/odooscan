from odoo import fields, models


class ResPartner(models.Model):
    _inherit = "res.partner"

    sa20_stage = fields.Selection([("draft", "Draft")], required=True, default="draft")
    sa20_phase = fields.Selection([("draft", "Draft")], required=True, default="draft")
    sa20_step = fields.Selection([("draft", "Draft"), ("done", "Done")], required=True, default="draft")
    sa20_grade = fields.Selection([("low", "Low")], required=True, default="low")
    sa20_level = fields.Selection([("low", "Low")], required=True, default="low")
    sa20_tier = fields.Selection([("low", "Low")])


class ResPartnerHeld(models.Model):
    _inherit = "res.partner"

    sa20_stage = fields.Selection(selection_add=[("held",)])
    sa20_phase = fields.Selection(selection_add=[("held",)], ondelete={"held": "cascade"})
    sa20_step = fields.Selection(selection_add=[("done",), ("draft",)])
    sa20_grade = fields.Selection(selection_add=(("high", "High"),), ondelete={"high": "cascade"})
    sa20_level = fields.Selection(selection_add=(("high", "High"),))
    sa20_tier = fields.Selection(selection_add=[("high",)])
