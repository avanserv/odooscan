from odoo import fields, models


class ChildAnyPlan(models.Model):
    _name = "child.any.plan"
    _description = "ChildAny plan"

    name = fields.Char()

    def action_plan(self):
        return True


class ChildAnyTask(models.Model):
    _name = "child.any.task"
    _description = "ChildAny task"

    name = fields.Char()

    def action_task(self):
        return True
