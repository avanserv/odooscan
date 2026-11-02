from odoo import fields, models


class OwnMixin(models.AbstractModel):
    _name = "inherited.b13.own"
    _description = "Own mixin"

    badge = fields.Char()


class Target(models.Model):
    _name = "inherited.b13.target"
    _description = "Target"

    gift = fields.Char()


class OwnForward(models.AbstractModel):
    _name = "inherited.b13.own.forward"
    _inherits = {"inherited.b13.target": "target_id"}
    _description = "Own forwarding mixin"

    target_id = fields.Many2one("inherited.b13.target", required=True, ondelete="cascade")


class Badged(models.Model):
    _name = "inherited.b13.badged"
    _inherit = "inherited.b13.own"
    _description = "Badged"

    field_id = fields.Many2one("ir.model.fields")


class Host(models.Model):
    _name = "inherited.b13.host"
    _inherit = ["inherited.b13.host", "inherited.b13.own", "inherited.b13.own.forward"]
