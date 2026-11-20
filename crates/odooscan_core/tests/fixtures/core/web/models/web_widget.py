from odoo import fields, models


def widget_model(kind):
    return ".".join(("web", "widget", kind))


class WebWidgetText(models.Model):
    _name = widget_model("text")

    name = fields.Char()
