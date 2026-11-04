from odoo import models


class OrderedSignature(models.AbstractModel):
    _name = "ordered.signature.mixin"
    _description = "Ordered signature mixin"

    def _ordered(self, order, line):
        return order, line
