from odoo import fields, models

KIND = "draft"


# The class keeps the last _name, a literal, so the file builds no model name.
class SettledHolder(models.Model):
    _name = f"settled.{KIND}"
    _name = "settled.holder"
    _description = "Settled holder"

    name = fields.Char()
