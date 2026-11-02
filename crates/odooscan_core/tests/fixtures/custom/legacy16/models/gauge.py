from odoo import fields, models


class LegacyGauge(models.Model):
    _name = "legacy.gauge"
    _description = "Legacy gauge"
    _auto = False

    level = fields.Float()


# Fields a class of `base` gives every model leave this one with nothing of its own to read.
class LegacyFieldlessGauge(models.Model):
    _name = "legacy.gauge.fieldless"
    _description = "Legacy fieldless gauge"
    _auto = False

    def reading(self):
        return 0


class LegacyBase(models.AbstractModel):
    _inherit = "base"

    legacy_mark = fields.Char()
