from odoo import fields, models


class RuleNote(models.Model):
    _name = "rule.note"
    _description = "Rule note"

    name = fields.Char()
