from odoo import fields, models


class AutoScopeThing(models.Model):
    _inherit = "auto.scope.thing"

    drag_stamp = fields.Char()
