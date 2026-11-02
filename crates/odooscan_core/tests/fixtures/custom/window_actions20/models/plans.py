from odoo import fields, models


class WindowDay(models.Model):
    _name = "window.day"
    _description = "Window day"

    date = fields.Date()
