from odoo import models
from odoo.addons.domain_bases.models import domain_models

from .domain_common import DomainOwn
from .domain_models import DomainStamped


class DomainRelative(DomainStamped):
    _name = "domain.relative"
    _inherit = ["domain.stamped"]
    _description = "Python base a relative import names, of the chain"

    def x_lookup(self):
        return self.search([("no_relative_stamp", "=", "a")])


class DomainModule(domain_models.DomainStamped):
    _name = "domain.module"
    _inherit = ["domain.stamped"]
    _description = "Python base an imported module holds, of the chain"

    def x_lookup(self):
        return self.search([("no_module_stamp", "=", "a")])


class Shadowed(models.Model):
    _name = "domain.shadowed"
    _description = "Holds no field, then gives its name to an import"


from .domain_common import DomainOwn as Shadowed


class DomainReimported(Shadowed):
    _name = "domain.reimported"
    _inherit = ["domain.shadowed"]
    _description = "Python base an import rebinds past a class of the chain"

    def x_lookup(self):
        return self.search([("stamp", "=", "a")])


class Assigned(models.Model):
    _inherit = "domain.shadowed"


Assigned = DomainOwn


class DomainAssigned(Assigned):
    _name = "domain.assigned"
    _inherit = ["domain.shadowed"]
    _description = "Python base an assignment rebinds past a class of the chain"

    def x_lookup(self):
        return self.search([("stamp", "=", "a")])
