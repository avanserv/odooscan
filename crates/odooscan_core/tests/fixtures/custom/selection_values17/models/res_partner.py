from odoo import fields, models


class ResPartner(models.Model):
    _inherit = "res.partner"

    sv17_mode = fields.Selection((("draft", "Draft"), ("done", "Done")))
    sv17_gate = fields.Selection((("open", "Open"),), required=True, default="open")
    sv17_agent_id = fields.Many2one("res.partner", company_dependent=True)
    sv17_grade = fields.Selection([("gold", "Gold"), ("silver", "Silver")], company_dependent=True)
    sv17_unstored_agent_id = fields.Many2one("res.partner", company_dependent=True, store=False)
    sv17_unstored_grade = fields.Selection([("gold", "Gold")], company_dependent=True, store=False)
    sv17_computed_agent_id = fields.Many2one("res.partner", company_dependent=True, compute="_compute_sv17_agent_id")
    sv17_computed_grade = fields.Selection([("gold", "Gold")], company_dependent=True, compute="_compute_sv17_grade")

    def _compute_sv17_agent_id(self):
        self.sv17_computed_agent_id = False

    def _compute_sv17_grade(self):
        self.sv17_computed_grade = "gold"


class ResPartnerGate(models.Model):
    _inherit = "res.partner"

    sv17_gate = fields.Selection(selection_add=[("shut", "Shut")])
