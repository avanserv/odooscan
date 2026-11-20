from odoo import models


class PrivateSteps(models.Model):
    _inherit = "private.signature"

    # Silent though it drops line: the plain base binds _PrivateSteps__stamp, unread.
    def __stamp(self, order):
        return order


class PrivateOther(models.Model):
    _inherit = "private.signature"

    # Reported: here __stamp compiles to _PrivateOther__stamp, which the plain base does not bind.
    def __stamp(self, order):
        return order
