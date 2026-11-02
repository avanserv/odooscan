from openerp import fields, models


class Tagged(models.Model):
    _name = "inherited.b9.tagged"
    _inherit = "inherited.b9.tag"
    _description = "Tagged"

    field_id = fields.Many2one("ir.model.fields")


class Plain(models.Model):
    _name = "inherited.b9.plain"
    _description = "Plain"

    name = fields.Char()


class Child(models.Model):
    _name = "inherited.b9.child"
    _inherits = {"inherited.b9.delegate": "delegate_id"}
    _description = "Child"

    delegate_id = fields.Many2one("inherited.b9.delegate", required=True, ondelete="cascade")


class Host(models.Model):
    _name = "inherited.b9.host"
    _inherit = ["inherited.b9.host", "inherited.b9.tag", "inherited.b9.forward"]


class Forwarded(models.Model):
    _name = "inherited.b9.forwarded"
    _inherit = "inherited.b9.forward"
    _description = "Forwarded"


class Late(models.Model):
    _name = "inherited.b9.late"
    _description = "Late"

    name = fields.Char()


class LateTagged(models.Model):
    _name = "inherited.b9.late"
    _inherit = ["inherited.b9.late", "inherited.b9.tag"]
