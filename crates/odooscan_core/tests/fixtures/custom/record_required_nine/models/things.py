from odoo import fields, models

DEFAULTS = {"code": "c"}


class RnLegacy(models.Model):
    _name = "rn.legacy"
    _description = "Legacy defaults by name"
    _defaults = {"code": "c"}

    code = fields.Char(required=True)
    other = fields.Char(required=True)


class RnLegacyHidden(models.Model):
    _name = "rn.legacy.hidden"
    _description = "Legacy defaults by expression"
    _defaults = DEFAULTS

    code = fields.Char(required=True)
