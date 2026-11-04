from odoo import fields, models


class SignatureSteps:
    def _stamp(self, order, line):
        return order, line


class MixedSignature(models.Model, SignatureSteps):
    _name = "mixed.signature"
    _description = "Mixed signature"

    name = fields.Char()

    def _prepare(self, order, line):
        return order, line


class MixedSignatureStamp(models.Model):
    _inherit = "mixed.signature"

    def _stamp(self, order, line):
        return super()._stamp(order, line)
