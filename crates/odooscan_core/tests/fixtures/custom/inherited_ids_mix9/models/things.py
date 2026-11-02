from openerp import fields, models


class ChainedMixin(models.AbstractModel):
    _name = "inherited.b9.chained"
    _description = "Chained mixin"

    deep = fields.Char()


class TagMixin(models.AbstractModel):
    _name = "inherited.b9.tag"
    _inherit = "inherited.b9.chained"
    _description = "Tag mixin"

    tag = fields.Char()


class Delegate(models.Model):
    _name = "inherited.b9.delegate"
    _description = "Delegate"

    title = fields.Char()


class ForwardMixin(models.AbstractModel):
    _name = "inherited.b9.forward"
    _inherits = {"inherited.b9.delegate": "delegate_id"}
    _description = "Forwarding mixin"

    delegate_id = fields.Many2one("inherited.b9.delegate", required=True, ondelete="cascade")


class Host(models.Model):
    _name = "inherited.b9.host"
    _description = "Host"

    name = fields.Char()
