from odoo import fields, models


class _PrivateSteps:
    def __stamp(self, order):
        return order


class PrivateSignature(models.Model, _PrivateSteps):
    _name = "private.signature"
    _description = "Private signature"

    name = fields.Char()


# The parents the extension's overrides are compared with; a plain base's signature is not read.
class PrivateSteps(models.Model):
    _inherit = "private.signature"

    def __stamp(self, order, line):
        return order, line


class PrivateOther(models.Model):
    _inherit = "private.signature"

    def __stamp(self, order, line):
        return order, line
