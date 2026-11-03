from odoo import fields, models

from .unindexed import SignatureMixin


class HiddenSignature(models.Model, SignatureMixin):
    _name = "hidden.signature"
    _description = "Hidden signature"

    name = fields.Char()

    def _prepare(self, order, line):
        return order, line
