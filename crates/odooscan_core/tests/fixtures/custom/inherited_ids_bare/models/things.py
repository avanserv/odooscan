from odoo import fields, models


class Tagged(models.Model):
    _name = "inherited.bare.tagged"
    _inherit = "inherited.bare.tag"
    _description = "Tagged"

    field_id = fields.Many2one("ir.model.fields")


class Plain(models.Model):
    _name = "inherited.bare.plain"
    _description = "Plain"

    name = fields.Char()


class Child(models.Model):
    _name = "inherited.bare.child"
    _inherits = {"inherited.bare.delegate": "delegate_id"}
    _description = "Child"

    delegate_id = fields.Many2one("inherited.bare.delegate", required=True, ondelete="cascade")


class Host(models.Model):
    _name = "inherited.bare.host"
    _inherit = ["inherited.bare.host", "inherited.bare.tag"]
