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


class SuperAttributes(models.Model):
    _inherit = "bad.super"

    def __getattr__(self, name):
        return super().__getattr__(name)

    def _lookup(self, name):
        return super().__getattr__(name)


class SuperSeal(models.Model):
    _inherit = "bad.super"

    def __seal(self):
        return 1


class SuperSealOther(models.Model):
    _inherit = "bad.super"

    def __seal(self):
        return super().__seal()


class SuperSeal(models.Model):
    _inherit = "bad.super"

    def __seal(self):
        return super(SuperSeal, self).__seal()


class _SuperSeal(models.Model):
    _inherit = "bad.super"

    def __seal(self):
        return super().__seal()


class SuperSteps:
    def _step(self):
        return 1

    def __call__(self):
        return 1

    def __step(self):
        return 1


class SuperStepped(models.Model, SuperSteps):
    _inherit = "bad.super"


class SuperStepCall(models.Model):
    _inherit = "bad.super"

    def _step(self):
        return super()._step()

    def __call__(self):
        return super().__call__()


class _SuperSteps(models.Model):
    _inherit = "bad.super"

    def __step(self):
        return super().__step()


class SuperNestedExplicit(models.Model):
    _inherit = "bad.super"

    def _nest_explicit(self):
        class Proxy(dict):
            gone = super(SuperNestedExplicit, self)._explicit_gone()

            def _later(self):
                return super(SuperNestedExplicit, self)._explicit_later()

        return Proxy


class SuperNestedDynamic(models.Model):
    _inherit = "bad.super"

    def _dyn_own(self):
        return 1

    def _nest_dynamic(self):
        class Typed(type(self)):
            def _typed(self):
                if hasattr(super(), "_dynamic_guard"):
                    super()._dynamic_guard()
                super()._dyn_own()
                super().action_go()
                return super()._dynamic_gone()

        class Classed(self.__class__):
            def _classed(self):
                return super()._class_gone()

        class Mixed(type(self), dict):
            def _mixed(self):
                return super()._mixed_gone()

        return Typed, Classed, Mixed

    @classmethod
    def _nest_meta(cls):
        class Meta(type(cls)):
            def _meta(self):
                return super()._meta_gone()

        return Meta


class SuperClassBody(models.Model):
    _inherit = "bad.super"

    def _body(self):
        class Holder(dict):
            gone = super()._body_gone()
            names = [super()._comp_gone() for _ in range(2)]

            def _later(self):
                return super().get("x")

        return Holder
