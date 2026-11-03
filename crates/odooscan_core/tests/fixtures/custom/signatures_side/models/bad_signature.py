from odoo import models


class BadSignature(models.Model):
    _inherit = "bad.signature"

    def _side(self, order, line):
        return order, line
