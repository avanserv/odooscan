from odoo import fields, models


class LostHook(models.Model):
    _name = "lost.hook"
    _description = "Lost hook"

    name = fields.Char()
