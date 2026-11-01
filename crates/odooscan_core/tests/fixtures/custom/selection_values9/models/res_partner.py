from odoo import fields, models


class ResPartner(models.Model):
    _inherit = "res.partner"

    sv9_mode = fields.Selection((("draft", "Draft"), ("done", "Done")))
    sv9_state = fields.Selection([("draft", "Draft"), ("done", "Done")])
    sv9_agent_id = fields.Many2one("res.partner", company_dependent=True)
    sv9_grade = fields.Selection([("gold", "Gold"), ("silver", "Silver")], company_dependent=True)
