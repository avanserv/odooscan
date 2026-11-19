from odoo import fields, models

HELD_VALUES = [("held", "Held")]


class ResPartner(models.Model):
    _inherit = "res.partner"

    sa171_stage = fields.Selection([("draft", "Draft")], required=True, default="draft")
    sa171_phase = fields.Selection([("draft", "Draft")], required=True, default="draft")
    sa171_level = fields.Selection([("draft", "Draft")], required=True, default="draft")
    sa171_grade = fields.Selection([("draft", "Draft")], required=True, default="draft")
    sa171_rank = fields.Selection([("draft", "Draft")], required=True, default="draft")
    sa171_scope = fields.Selection([("draft", "Draft")], required=True, default="draft")
    sa171_mode = fields.Selection([("draft", "Draft")], required=True, default="draft")


class ResPartnerHeld(models.Model):
    _inherit = "res.partner"

    sa171_stage = fields.Selection(selection_add=[("held",)], ondelete={"held": "cascade"})
    sa171_phase = fields.Selection(selection_add=[("held",)], ondelete={"held": "cascade"})
    sa171_level = fields.Selection(selection_add=[("held",)], ondelete={"held": "cascade"})
    sa171_grade = fields.Selection(selection_add=[("held",)], ondelete={"held": "cascade"})
    sa171_rank = fields.Selection(selection_add=[("held",)], ondelete={"held": "cascade"})
    sa171_scope = fields.Selection(selection_add=[("held",)], ondelete={"held": "cascade"})
    sa171_mode = fields.Selection(selection_add=[("held",)], ondelete={"held": "cascade"})


class ResPartnerLabel(models.Model):
    _inherit = "res.partner"

    sa171_stage = fields.Selection(selection_add=[("held", "Held")], ondelete={"held": "cascade"})
    sa171_phase = fields.Selection(selection=[("draft", "Draft"), ("done", "Done")])
    sa171_level = fields.Selection(selection_add=[("held",)], ondelete={"held": "cascade"})
    sa171_rank = fields.Selection(selection_add=[("held", "Held", "On hold")], ondelete={"held": "cascade"})
    sa171_scope = fields.Selection(selection_add=HELD_VALUES, ondelete={"held": "cascade"})
    sa171_mode = fields.Selection(selection_add=["held"], ondelete={"held": "cascade"})
