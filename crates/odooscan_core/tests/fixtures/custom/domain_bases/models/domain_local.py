from odoo import fields, models


class Base(models.AbstractModel):
    _inherit = "base"


class DomainLocal(models.Model, Base):
    _name = "domain.local"
    _description = "Python base of the base model, in the same file"

    def x_lookup(self):
        return self.search([("no_base_class", "=", "a")])


class DomainTwice(models.Model):
    _name = "domain.twice.first"
    _description = "Replaced in the file by the next class of that name"


class DomainTwice(models.Model):
    _name = "domain.twice.last"
    _description = "The class the name holds below"

    stamp = fields.Char()


class DomainRebound(DomainTwice):
    _name = "domain.rebound"
    _inherit = ["domain.twice.first"]
    _description = "Python base that _inherit does not read"

    def x_lookup(self):
        return self.search([("stamp", "=", "a")])
