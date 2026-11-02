from odoo import fields, models


class AutoScopeThing(models.Model):
    _inherit = "auto.scope.thing"

    country_any_stamp = fields.Char()
