from odoo import fields, models


class Tagged(models.Model):
    _name = "inherited.b13.tagged"
    _inherit = "inherited.b13.tag"
    _description = "Tagged"

    field_id = fields.Many2one("ir.model.fields")


class Plain(models.Model):
    _name = "inherited.b13.plain"
    _description = "Plain"

    name = fields.Char()


class Child(models.Model):
    _name = "inherited.b13.child"
    _inherits = {"inherited.b13.delegate": "delegate_id"}
    _description = "Child"

    delegate_id = fields.Many2one("inherited.b13.delegate", required=True, ondelete="cascade")


class Host(models.Model):
    _name = "inherited.b13.host"
    _inherit = ["inherited.b13.host", "inherited.b13.tag", "inherited.b13.forward"]


class Forwarded(models.Model):
    _name = "inherited.b13.forwarded"
    _inherit = "inherited.b13.forward"
    _description = "Forwarded"


class Late(models.Model):
    _name = "inherited.b13.late"
    _description = "Late"

    name = fields.Char()


class LateTagged(models.Model):
    _name = "inherited.b13.late"
    _inherit = ["inherited.b13.late", "inherited.b13.tag"]
