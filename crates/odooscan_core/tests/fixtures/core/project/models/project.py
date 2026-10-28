from odoo import fields, models


class Project(models.Model):
    _name = "project.project"

    name = fields.Char()


class Task(models.Model):
    _name = "project.task"

    name = fields.Char()
    project_id = fields.Many2one("project.project")
