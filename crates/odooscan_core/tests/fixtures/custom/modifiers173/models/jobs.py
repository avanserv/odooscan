from odoo import fields, models


class ModifierTask(models.Model):
    _name = "modifier.task"
    _description = "Modifier task"

    name = fields.Char()
    state = fields.Char()
    note = fields.Char()
    user_id = fields.Many2one("res.users")
    start = fields.Date()
