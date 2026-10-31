from odoo import fields, models


class ModifierJob(models.Model):
    _name = "modifier.job"
    _description = "Modifier job"

    name = fields.Char()
    state = fields.Char()
    note = fields.Char()
    user_id = fields.Many2one("res.users")
    start = fields.Date()
    step_ids = fields.One2many("modifier.step", "job_id")
    lead_id = fields.Many2one("modifier.step")


class ModifierStep(models.Model):
    _name = "modifier.step"
    _description = "Modifier step"

    name = fields.Char()
    phase = fields.Char()
    start = fields.Date()
    job_id = fields.Many2one("modifier.job")
