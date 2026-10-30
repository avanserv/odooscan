from odoo import fields, models


class Task(models.Model):
    _name = "project.task"

    name = fields.Char()
    project_id = fields.Many2one("project.project")
