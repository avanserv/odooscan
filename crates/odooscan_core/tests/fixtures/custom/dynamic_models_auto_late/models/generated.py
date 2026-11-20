from odoo import fields, models

for kind in ["large", "small"]:

    class AutoLate(models.Model):
        _name = f"auto.late.{kind}"

        size = fields.Integer()
