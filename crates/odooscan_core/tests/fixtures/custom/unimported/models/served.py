from odoo import http, models


class Served(http.Controller):
    pass


class PartnerServed(models.Model):
    _inherit = "res.partner"
