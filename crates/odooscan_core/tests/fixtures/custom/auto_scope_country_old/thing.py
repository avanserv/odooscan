from odoo import fields, models


class AutoScopeThing(models.Model):
    _inherit = "auto.scope.thing"

    country_old_stamp = fields.Char()
