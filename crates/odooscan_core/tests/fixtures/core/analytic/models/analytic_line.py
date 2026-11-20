from odoo import fields, models


class AnalyticLine(models.Model):
    _name = "account.analytic.line"

    name = fields.Char()
