from odoo import fields, models


class HookReport(models.Model):
    _name = "hook.report"
    _description = "Hook report"
    _auto = False

    total = fields.Integer()
