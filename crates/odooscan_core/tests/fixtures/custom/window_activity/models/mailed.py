from odoo import fields, models


class WindowMailed(models.Model):
    _name = "window.mailed"
    _inherit = ["mail.activity.mixin"]
    _description = "Window mailed"

    name = fields.Char()


class WindowScheduled(models.Model):
    _name = "window.scheduled"
    _inherit = ["window.scheduled", "mail.activity.mixin"]


class WindowNoted(models.Model):
    _name = "window.noted"
    _description = "Window noted"

    name = fields.Char()


class WindowBoxed(models.Model):
    _name = "window.boxed"
    _description = "Window boxed"

    name = fields.Char()
