from odoo import fields, models


class ModifierJob(models.Model):
    _name = "modifier.job"
    _description = "Modifier job"

    name = fields.Char()
    state = fields.Char()
    note = fields.Char()
    user_id = fields.Many2one("res.users")
    start = fields.Date()
