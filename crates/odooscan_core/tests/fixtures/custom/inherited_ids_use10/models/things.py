from odoo import fields, models


class Tagged(models.Model):
    _name = "inherited.b10.tagged"
    _inherit = "inherited.b10.tag"
    _description = "Tagged"

    field_id = fields.Many2one("ir.model.fields")


class Plain(models.Model):
    _name = "inherited.b10.plain"
    _description = "Plain"

    name = fields.Char()


class Child(models.Model):
    _name = "inherited.b10.child"
    _inherits = {"inherited.b10.delegate": "delegate_id"}
    _description = "Child"

    delegate_id = fields.Many2one("inherited.b10.delegate", required=True, ondelete="cascade")


class Host(models.Model):
    _name = "inherited.b10.host"
    _inherit = ["inherited.b10.host", "inherited.b10.tag", "inherited.b10.forward"]


class Forwarded(models.Model):
    _name = "inherited.b10.forwarded"
    _inherit = "inherited.b10.forward"
    _description = "Forwarded"


class Late(models.Model):
    _name = "inherited.b10.late"
    _description = "Late"

    name = fields.Char()


class LateTagged(models.Model):
    _name = "inherited.b10.late"
    _inherit = ["inherited.b10.late", "inherited.b10.tag"]
