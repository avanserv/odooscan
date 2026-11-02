from odoo import fields, models


class WindowNestedNew(models.Model):
    _name = "window.nested.new"
    _description = "Window nested 15.1"

    name = fields.Char()
