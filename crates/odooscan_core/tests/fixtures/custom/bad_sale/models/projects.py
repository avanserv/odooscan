from odoo import fields, models


class SaleOrderLineProject(models.Model):
    _inherit = "bad.order.line"

    project_id = fields.Many2one("project.project")
    task_id = fields.Many2one("project.task")
    timesheet_id = fields.Many2one("account.analytic.line")
