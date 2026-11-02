from odoo import fields, models


class WindowSlot(models.Model):
    _name = "window.slot"
    _description = "Window slot"

    name = fields.Char()
