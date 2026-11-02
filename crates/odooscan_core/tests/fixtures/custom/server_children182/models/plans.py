from odoo import fields, models


class Child182Plan(models.Model):
    _name = "child182.plan"
    _description = "Child182 plan"

    name = fields.Char()

    def action_plan(self):
        return True


class Child182Task(models.Model):
    _name = "child182.task"
    _description = "Child182 task"

    name = fields.Char()

    def action_task(self):
        return True
