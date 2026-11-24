from odoo import fields, models


class WindowClearedOld(models.Model):
    _name = "window.cleared.old"
    _description = "Window cleared 12"

    name = fields.Char()
