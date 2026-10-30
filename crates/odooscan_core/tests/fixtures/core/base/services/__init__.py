from odoo import fields, models

from . import label_service


class LabelQueue(models.Model):
    _name = "x.label.queue"

    name = fields.Char()
