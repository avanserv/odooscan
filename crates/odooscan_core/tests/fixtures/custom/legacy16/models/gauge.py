from odoo import fields, models


class LegacyGauge(models.Model):
    _name = "legacy.gauge"
    _description = "Legacy gauge"
    _auto = False

    level = fields.Float()
