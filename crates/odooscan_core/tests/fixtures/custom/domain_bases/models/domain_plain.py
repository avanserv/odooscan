import logging

from odoo import fields, models

from .domain_mixins import ImportedStamp


class Fieldless(object):
    __slots__ = ()

    def x_label(self):
        return "a"


class StampHolder:
    stamp = fields.Char()


class DomainFieldless(models.TransientModel, Fieldless):
    _name = "domain.fieldless"
    _description = "Python base with no field to give"

    def x_lookup(self):
        return self.search([("no_fieldless", "=", "a")])


# From saas~14.4 Odoo refuses a field on a class that is no model, so stamp cannot reach the model.
class DomainHolding(models.Model, StampHolder):
    _name = "domain.holding"
    _description = "Python base with a field"

    def x_lookup(self):
        return self.search([("stamp", "=", "a")])


class DomainForeign(models.Model, ImportedStamp):
    _name = "domain.foreign"
    _description = "Python base of another file"

    def x_lookup(self):
        return self.search([("stamp", "=", "a")])


class Valued:
    sizes = {"small": 1, "large": 2}
    kinds = {"small", "large"}
    log = logging.getLogger(__name__)


class Hooked:
    def __init_subclass__(cls, **kwargs):
        super().__init_subclass__(**kwargs)


class Delegating:
    _inherits = {"domain.stamped": "stamped_id"}


class DomainValued(models.Model, Valued):
    _name = "domain.valued"
    _description = "Python base with values that hold no field"

    def x_lookup(self):
        return self.search([("no_valued", "=", "a")])


class DomainHooked(models.Model, Hooked):
    _name = "domain.hooked"
    _description = "Python base with a hook that adds no field"

    def x_lookup(self):
        return self.search([("no_hooked", "=", "a")])


class DomainDelegating(Delegating, models.Model):
    _name = "domain.delegating"
    _description = "Python base that delegates to another model"

    stamped_id = fields.Many2one("domain.stamped", required=True, ondelete="cascade")

    def x_lookup(self):
        return self.search([("no_delegating", "=", "a")])
