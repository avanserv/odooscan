from odoo import fields, models


class RuleTicket(models.Model):
    _name = "rule.ticket"
    _description = "Rule ticket"

    name = fields.Char()
    user_id = fields.Many2one("res.users")
