from odoo import fields, models


class AutoScopeThing(models.Model):
    _inherit = "auto.scope.thing"

    far_stamp = fields.Char()
