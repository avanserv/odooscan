from openerp import fields, models


class OwnMixin(models.AbstractModel):
    _name = "inherited.b9.own"
    _description = "Own mixin"

    badge = fields.Char()


class Target(models.Model):
    _name = "inherited.b9.target"
    _description = "Target"

    gift = fields.Char()


class OwnForward(models.AbstractModel):
    _name = "inherited.b9.own.forward"
    _inherits = {"inherited.b9.target": "target_id"}
    _description = "Own forwarding mixin"

    target_id = fields.Many2one("inherited.b9.target", required=True, ondelete="cascade")


class Badged(models.Model):
    _name = "inherited.b9.badged"
    _inherit = "inherited.b9.own"
    _description = "Badged"

    field_id = fields.Many2one("ir.model.fields")


class Host(models.Model):
    _name = "inherited.b9.host"
    _inherit = ["inherited.b9.host", "inherited.b9.own", "inherited.b9.own.forward"]
