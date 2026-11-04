from odoo import models


class ResPartnerSignature(models.Model):
    _inherit = "res.partner"

    def _x_sign(self, order, line):
        return order, line


class ResPartnerSigned(models.Model):
    _inherit = "res.partner"

    def _x_sign(self, order):
        return super()._x_sign(order, False)
