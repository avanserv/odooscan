from odoo import fields, models


class WindowTagOld(models.Model):
    _name = "window.tag.old"
    _description = "Window tag old"

    name = fields.Char()


class WindowTagOther(models.Model):
    _name = "window.tag.other"
    _description = "Window tag other"

    name = fields.Char()
