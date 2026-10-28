from odoo import models


class PlainMixin:
    def _hook(self):
        return 1


class Super(models.Model):
    _name = "bad.super"

    def action_go(self):
        return True


class SuperExtension(models.Model):
    _inherit = "bad.super"

    def write(self, vals):
        return super().write(vals)

    def _compute_display_name(self):
        return super()._compute_display_name()

    def action_go(self):
        return super().action_go()

    def _prepare_gone(self):
        return super()._prepare_gone()


class SuperWithPlainBase(models.Model, PlainMixin):
    _inherit = "bad.super"

    def _hook(self):
        return super()._hook()


class SuperMixin(models.AbstractModel):
    _name = "bad.super.mixin"

    def _hook(self):
        return super()._hook()
