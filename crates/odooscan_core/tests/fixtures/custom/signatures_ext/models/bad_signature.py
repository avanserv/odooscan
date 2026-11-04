from odoo import models


def logged(method):
    return method


class BadSignature(models.Model):
    _inherit = "bad.signature"

    def _prepare(self, order):
        return super()._prepare(order, False)

    def _stamp(self, order, extra):
        return super()._stamp(order)

    def _render(self, *, style):
        return super()._render(mode=style)

    def _mark(self, *, flag):
        return super()._mark()

    def _spread(self, order):
        return super()._spread(order)

    def _gather(self, *args, **kwargs):
        return super()._gather(*args, **kwargs)

    def _trim(self, order):
        return super()._trim(order)

    def _harden(self, order, line):
        return super()._harden(order, line)

    def _extend(self, order, extra=None):
        return super()._extend(order)

    @staticmethod
    def _static(order, line):
        return order, line

    def _defaults(self, order, line):
        return super()._defaults(order, line)

    def _wrapped(self, order):
        return super()._wrapped(order, False)

    @logged
    def _plain(self, order):
        return super()._plain(order, False)

    def _branch(self, order):
        return super()._branch(order, False)

    def _side(self, order):
        return order

    def _label(self, *, style=None):
        return super()._label(mode=style)

    def _twice(self, order, line):
        return super()._twice(order, line)

    def __seal(self, order):
        return order

    def __vouch(self, order, line):
        return order, line

    def _keyed(self, order, line):
        return super()._keyed(order, line=line)

    def _split(self, order):
        return order

    def _ordered(self, order, line):
        return super()._ordered(order, line)


class BadSignatureAgain(models.Model):
    _inherit = "bad.signature"

    def _prepare(self, order):
        return super()._prepare(order)

    def _twice(self, order):
        return super()._twice(order, False)

    def __vouch(self, order, line, extra):
        return order, line, extra

    def _ordered(self, order):
        return super()._ordered(order, False)


class BadSignatureCopy(models.Model):
    _name = "bad.signature.copy"
    _inherit = "bad.signature"
    _description = "Bad signature copy"

    def _prepare(self, order):
        return super()._prepare(order, False)
