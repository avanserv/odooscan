from odoo import fields, models


class Project(models.Model):
    _inherit = "project.project"

    allow_timesheets = fields.Boolean()


class AnalyticLine(models.Model):
    _inherit = "account.analytic.line"

    task_id = fields.Many2one("project.task")
