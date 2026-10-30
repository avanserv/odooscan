from odoo import fields, models


class LabelWizard(models.TransientModel):
    _name = "x.label.wizard"

    copies = fields.Integer()


class LabelFormat:
    margin = 2
