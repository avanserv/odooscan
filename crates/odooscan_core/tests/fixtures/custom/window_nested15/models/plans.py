from odoo import fields, models


class WindowNestedOld(models.Model):
    _name = "window.nested.old"
    _description = "Window nested 15"

    name = fields.Char()
