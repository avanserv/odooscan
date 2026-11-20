from odoo import fields, models


class SignatureSteps:
    def _stamp(self, order, line):
        return order, line

    def __call__(self, order):
        return order


class MixedSignature(models.Model, SignatureSteps):
    _name = "mixed.signature"
    _description = "Mixed signature"

    name = fields.Char()

    def _prepare(self, order, line):
        return order, line


# Its _stamp and __call__ differ from the extension's, so only the plain base's bindings keep
# the extension's overrides of those two silent.
class MixedSignatureStamp(models.Model):
    _inherit = "mixed.signature"

    def _stamp(self, order, line):
        return super()._stamp(order, line)

    def __call__(self, order, line):
        return order, line
