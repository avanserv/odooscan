from odoo import fields, models


class ModernDigest(models.Model):
    _name = "modern.digest"
    _description = "Modern digest"
    _auto = False

    entry_count = fields.Integer()


class ModernGauge(models.Model):
    _name = "modern.gauge"
    _description = "Modern gauge"
    _auto = False

    level = fields.Float()
