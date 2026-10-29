from odoo import fields, models


class HookSuffixReport(models.Model):
    _name = "hook.suffix.report"
    _description = "Hook suffix report"
    _auto = False

    total = fields.Integer()
