from odoo import fields, models


class ChildPlan(models.Model):
    _name = "child.plan"
    _description = "Child plan"

    name = fields.Char()

    def action_plan(self):
        return True


class ChildTask(models.Model):
    _name = "child.task"
    _description = "Child task"

    name = fields.Char()

    def action_task(self):
        return True
