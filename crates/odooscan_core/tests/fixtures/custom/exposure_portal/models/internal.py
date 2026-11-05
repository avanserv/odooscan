from odoo import fields, models


class ExposureInternal(models.Model):
    _inherit = "exposure.internal"

    body = fields.Html(string="Message")
    relay_token = fields.Char()


class ExposureBoard(models.Model):
    _inherit = "exposure.board"

    body = fields.Html(sanitize=True)
