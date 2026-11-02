from odoo import fields, models


class Tagged(models.Model):
    _name = "inherited.b14.tagged"
    _inherit = "inherited.b14.tag"
    _description = "Tagged"

    field_id = fields.Many2one("ir.model.fields")


class Plain(models.Model):
    _name = "inherited.b14.plain"
    _description = "Plain"

    name = fields.Char()


class Child(models.Model):
    _name = "inherited.b14.child"
    _inherits = {"inherited.b14.delegate": "delegate_id"}
    _description = "Child"

    delegate_id = fields.Many2one("inherited.b14.delegate", required=True, ondelete="cascade")


class Host(models.Model):
    _name = "inherited.b14.host"
    _inherit = ["inherited.b14.host", "inherited.b14.tag", "inherited.b14.forward"]


class Forwarded(models.Model):
    _name = "inherited.b14.forwarded"
    _inherit = "inherited.b14.forward"
    _description = "Forwarded"


class Late(models.Model):
    _name = "inherited.b14.late"
    _description = "Late"

    name = fields.Char()


class LateTagged(models.Model):
    _name = "inherited.b14.late"
    _inherit = ["inherited.b14.late", "inherited.b14.tag"]
