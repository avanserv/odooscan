from odoo import fields, models


class BadGeneratedLarge(models.Model):
    _inherit = ["res.partner"]

    note = fields.Char()


class UnknownHolder(models.Model):
    _name = "unknown.holder"

    large_id = fields.Many2one("bad.generated.large")
    huge_id = fields.Many2one("bad.generated.huge")
    line_id = fields.Many2one("side.order.line")
    note_id = fields.Many2one("sideline.note")


class BadGeneratedHuge(models.Model):
    _inherit = ["bad.generated.huge", "res.partner"]


class Side(models.Model):
    _inherit = ["side.order"]
