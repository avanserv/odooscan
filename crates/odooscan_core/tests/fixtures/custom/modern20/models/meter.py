from odoo import fields, models


class ModernMeter(models.Model):
    _name = "modern.meter"
    _description = "Modern meter"
    _auto = False

    reading = fields.Integer()
