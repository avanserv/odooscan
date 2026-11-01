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
