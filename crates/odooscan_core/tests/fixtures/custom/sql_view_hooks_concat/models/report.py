from odoo import fields, models


class HookConcatReport(models.Model):
    _name = "hook.concat.report"
    _description = "Hook concat report"
    _auto = False

    total = fields.Integer()
