from odoo import fields, models


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


class SuperTrace(models.AbstractModel):
    _name = "bad.super.trace"
    _description = "Super trace"

    def _trace(self):
        return 1


class SuperParent(models.Model):
    _name = "bad.super.parent"
    _description = "Super parent"

    def _stamp(self):
        return 1


class SuperChild(models.Model):
    _name = "bad.super.child"
    _description = "Super child"
    _inherit = ["bad.super.trace"]
    _inherits = {"bad.super.parent": "parent_id"}

    parent_id = fields.Many2one("bad.super.parent", required=True, ondelete="cascade")

    def _trace(self):
        return super()._trace()

    def _stamp(self):
        return super()._stamp()


class SuperNested(models.Model):
    _inherit = "bad.super"

    def _nest(self):
        class Proxy(dict):
            def _nested_gone(self):
                return super()._nested_gone()

        return Proxy, super()._own_gone()


class SuperGuarded(models.Model):
    _inherit = "bad.super"

    def _guarded(self):
        if hasattr(super(), "_guard_gone"):
            super()._guard_gone()
        if hasattr(super(SuperGuarded, self), "_guard_old"):
            super(SuperGuarded, self)._guard_old()
        if hasattr(super(), "_guard_other"):
            super()._guard_unchecked()
        return super()._guard_gone()
