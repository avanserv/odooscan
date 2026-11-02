from odoo import fields, models


class WindowExtended(models.Model):
    _inherit = "window.extended"

    date_start = fields.Date()
    date_stop = fields.Date()
