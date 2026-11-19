from odoo import fields, models


class HeirHolder(models.Model):
    _name = "heir.holder"

    large_id = fields.Many2one("bad.generated.large")
    order_id = fields.Many2one("side.order")
    tiny_id = fields.Many2one("bad.generated.tiny")
    ghost_id = fields.Many2one("heir.ghost")
    note_id = fields.Many2one("x_heir_note")

    def _lookups(self):
        return (
            self.env["bad.generated.tiny"],
            self.env["heir.ghost"],
            self.env["x_heir.note"],
        )
