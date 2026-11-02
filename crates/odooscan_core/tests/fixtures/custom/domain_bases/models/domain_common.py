from odoo import fields, models


class Base(models.Model):
    _name = "domain.common"
    _description = "Same class name as the base model"

    stamp = fields.Char()


class ResPartner(models.Model):
    _name = "domain.namesake.parent"
    _description = "Same class name as a class of res.partner"

    stamp = fields.Char()


class DomainOwn(models.Model):
    _name = "domain.own.parent"
    _description = "Same class name as the class that imports it"

    stamp = fields.Char()


class DomainStamped(models.Model):
    _name = "domain.dotted.parent"
    _description = "Same class name as a class of the importing file"

    dotted_stamp = fields.Char()
