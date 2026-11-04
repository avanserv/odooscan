from odoo import models


class MixedSignature(models.Model):
    _inherit = "mixed.signature"

    # Reported: the plain base of the model's class is read, so every definition is known.
    def _prepare(self, order):
        return super()._prepare(order, False)

    # Silent: the plain base binds _stamp, whose signature the scan does not read.
    def _stamp(self, order):
        return super()._stamp(order, False)
