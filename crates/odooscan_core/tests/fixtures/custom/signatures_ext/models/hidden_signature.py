from odoo import models


class HiddenSignature(models.Model):
    _inherit = "hidden.signature"

    def _prepare(self, order):
        return super()._prepare(order, False)
