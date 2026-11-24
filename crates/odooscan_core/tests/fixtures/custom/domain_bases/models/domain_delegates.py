from odoo import fields, models

UNREAD_PARENTS = {"domain.stamped": "stamped_id"}
STAMPED = "domain.stamped"
DELEGATE = True
SPLAT = {"required": True, "ondelete": "cascade"}


class DomainUnreadInherits(models.Model):
    _name = "domain.unread.inherits"
    _inherits = UNREAD_PARENTS
    _description = "Computed _inherits"

    stamped_id = fields.Many2one("domain.stamped", required=True, ondelete="cascade")

    def x_lookup(self):
        return self.search([("stamp", "=", "a")])


class DomainUnreadFlag(models.Model):
    _name = "domain.unread.flag"
    _description = "Computed delegate flag"

    stamped_id = fields.Many2one(
        "domain.stamped", delegate=DELEGATE, required=True, ondelete="cascade"
    )

    def x_lookup(self):
        return self.search([("stamp", "=", "a")])


class DomainUnreadComodel(models.Model):
    _name = "domain.unread.comodel"
    _description = "Computed delegate comodel"

    stamped_id = fields.Many2one(STAMPED, delegate=True, required=True, ondelete="cascade")

    def x_lookup(self):
        return self.search([("stamp", "=", "a")])


class DomainUnreadKey(models.Model):
    _name = "domain.unread.key"
    _inherits = {STAMPED: "stamped_id"}
    _description = "Computed _inherits key"

    stamped_id = fields.Many2one("domain.stamped", required=True, ondelete="cascade")

    def x_lookup(self):
        return self.search([("stamp", "=", "a")])


class DomainUnreadAbove(models.Model):
    _name = "domain.unread.above"
    _inherits = {"domain.unread.flag": "flag_id"}
    _description = "Literal _inherits of a computed delegate"

    flag_id = fields.Many2one("domain.unread.flag", required=True, ondelete="cascade")

    def x_lookup(self):
        return self.search([("stamp", "=", "a")])


class DomainLiteralDelegate(models.Model):
    _name = "domain.literal.delegate"
    _inherits = {"domain.stamped": "stamped_id"}
    _description = "Literal _inherits, read"

    stamped_id = fields.Many2one("domain.stamped", required=True, ondelete="cascade")
    unread_id = fields.Many2one("domain.unread.inherits")

    def x_lookup(self):
        self.search([("stamp", "=", "a")])
        self.search([("unread_id.stamp", "=", "a")])
        return self.search([("no_literal_stamp", "=", "a")])


class DomainSplat(models.Model):
    _name = "domain.splat"
    _description = "Splat with no delegate"

    stamped_id = fields.Many2one("domain.stamped", **SPLAT)
    splat_stamp = fields.Char(related="stamp")

    def x_lookup(self):
        return self.search([("stamp", "=", "a")])
