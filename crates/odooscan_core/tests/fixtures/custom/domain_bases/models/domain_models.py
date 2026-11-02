from odoo import fields, models

from . import domain_common
from .domain_common import Base, DomainOwn, ResPartner

STAMPED = "domain.stamped"


class DomainStamped(models.Model):
    _name = "domain.stamped"
    _description = "Holds the field a Python base gives"

    stamp = fields.Char()


class DomainHidden(DomainStamped):
    _name = "domain.hidden"
    _description = "Python base, read by no _inherit"

    whole_id = fields.Many2one("domain.whole")

    def x_lookup(self):
        self.search([("whole_id.no_past_base", "=", "a")])
        return self.search([("stamp", "=", "a")])


class DomainComputed(models.Model):
    _name = "domain.computed"
    _inherit = [STAMPED]
    _description = "Computed _inherit"

    def x_lookup(self):
        return self.search([("stamp", "=", "a")])


class DomainDelegate(models.Model):
    _name = "domain.delegate"
    _inherits = {"domain.hidden": "hidden_id"}
    _description = "Delegates to a model with a Python base"

    hidden_id = fields.Many2one("domain.hidden", required=True, ondelete="cascade")

    def x_lookup(self):
        return self.search([("stamp", "=", "a")])


class DomainChained(DomainStamped):
    _name = "domain.chained"
    _inherit = ["domain.stamped"]
    _description = "Python base that _inherit reads too"

    def x_lookup(self):
        return self.search([("no_chained", "=", "a")])


class DomainImported(Base):
    _name = "domain.imported"
    _description = "Imported base named as the class of base"

    def x_lookup(self):
        return self.search([("stamp", "=", "a")])


class DomainNamesake(ResPartner):
    _name = "domain.namesake"
    _inherits = {"res.partner": "partner_id"}
    _description = "Imported base named as a class of res.partner"

    partner_id = fields.Many2one("res.partner", required=True, ondelete="cascade")

    def x_lookup(self):
        return self.search([("stamp", "=", "a")])


class DomainOwn(DomainOwn, models.Model):
    _name = "domain.own"
    _description = "Imported base named as this class"

    def x_lookup(self):
        return self.search([("stamp", "=", "a")])


class DomainDotted(domain_common.DomainStamped):
    _name = "domain.dotted"
    _inherit = ["domain.stamped"]
    _description = "Imported base named as a class of the chain above"

    def x_lookup(self):
        return self.search([("dotted_stamp", "=", "a")])


class DomainFound(DomainStamped):
    _name = "domain.found"
    _description = "Python base, with fields the path finds"

    label = fields.Char()

    def x_lookup(self):
        self.search([("label.no_past_char", "=", "a")])
        return self.search([("side_stamp", "=", "a")])


class DomainWhole(models.Model):
    _name = "domain.whole"
    _description = "Every parent read"

    hidden_id = fields.Many2one("domain.hidden", domain=[("stamp", "=", "a")])
    parent_id = fields.Many2one("domain.whole", domain=[("no_field_domain", "=", "a")])

    def x_lookup(self):
        self.search([("no_search", "=", "a")])
        return self.search([("hidden_id.stamp", "=", "a")])
