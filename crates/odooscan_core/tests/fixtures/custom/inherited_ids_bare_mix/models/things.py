from odoo import fields, models


class ChainedMixin(models.AbstractModel):
    _name = "inherited.bare.chained"
    _description = "Chained mixin"

    deep = fields.Char()


class TagMixin(models.AbstractModel):
    _name = "inherited.bare.tag"
    _inherit = "inherited.bare.chained"
    _description = "Tag mixin"

    tag = fields.Char()


class Delegate(models.Model):
    _name = "inherited.bare.delegate"
    _description = "Delegate"

    title = fields.Char()


class Host(models.Model):
    _name = "inherited.bare.host"
    _description = "Host"

    name = fields.Char()
