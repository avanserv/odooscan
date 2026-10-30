from odoo import fields, models


class WindowLoose(models.Model):
    _name = "window.loose"
    _description = "Window loose"

    date = fields.Date()
