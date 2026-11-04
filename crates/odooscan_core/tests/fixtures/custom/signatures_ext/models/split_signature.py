from odoo import models


class BadSignatureSplit(models.Model):
    _inherit = "bad.signature"

    def _split(self, order, line):
        return order, line
