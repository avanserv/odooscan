from odoo import fields, models


class ResPartner(models.Model):
    _inherit = "res.partner"

    web_color = fields.Char()


class WebTheme(models.Model):
    _name = "web.theme"

    name = fields.Char()
