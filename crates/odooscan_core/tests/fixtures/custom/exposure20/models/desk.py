from odoo import fields, models


class ExposureDesk(models.Model):
    _name = "exposure20.desk"
    _description = "Desk every user reads"

    name = fields.Char()
    body = fields.Html(sanitize=False)
    api_key = fields.Char()
