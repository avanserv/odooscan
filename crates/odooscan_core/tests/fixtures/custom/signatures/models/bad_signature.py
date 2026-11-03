from odoo import api, fields, models, release


def logged(method):
    return method


class BadSignature(models.Model):
    _name = "bad.signature"
    _description = "Bad signature"

    name = fields.Char()

    def _prepare(self, order, line):
        return {"order": order, "line": line}

    def _stamp(self, order):
        return order

    def _render(self, *, mode=None):
        return mode

    def _mark(self):
        return True

    def _spread(self, *args, **kwargs):
        return args, kwargs

    def _gather(self, order, line):
        return order, line

    def _trim(self, order, line=None):
        return order, line

    def _harden(self, order, line=None):
        return order, line

    def _extend(self, order):
        return order

    def _static(self, order, line):
        return order, line

    @api.model
    def _defaults(self, order, line):
        return order, line

    @logged
    def _wrapped(self, order, line):
        return order, line

    def _plain(self, order, line):
        return order, line

    def _label(self, *, mode=None):
        return mode

    def _twice(self, order, line):
        return order, line

    def __seal(self, order, line):
        return order, line

    def _keyed(self, order, *, line=None):
        return order, line

    if release.version_info >= (18, 0):

        def _branch(self, order, line):
            return order, line
