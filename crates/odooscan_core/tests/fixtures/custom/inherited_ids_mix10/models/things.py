from odoo import fields, models


class ChainedMixin(models.AbstractModel):
    _name = "inherited.b10.chained"
    _description = "Chained mixin"

    deep = fields.Char()


class TagMixin(models.AbstractModel):
    _name = "inherited.b10.tag"
    _inherit = "inherited.b10.chained"
    _description = "Tag mixin"

    tag = fields.Char()


class Delegate(models.Model):
    _name = "inherited.b10.delegate"
    _description = "Delegate"

    title = fields.Char()


class ForwardMixin(models.AbstractModel):
    _name = "inherited.b10.forward"
    _inherits = {"inherited.b10.delegate": "delegate_id"}
    _description = "Forwarding mixin"

    delegate_id = fields.Many2one("inherited.b10.delegate", required=True, ondelete="cascade")


class Host(models.Model):
    _name = "inherited.b10.host"
    _description = "Host"

    name = fields.Char()
