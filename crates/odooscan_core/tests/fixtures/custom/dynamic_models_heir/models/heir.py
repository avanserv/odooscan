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


class HeirPaths(models.Model):
    _inherit = "heir.holder"
    _rec_names_search = ["large_id.size"]

    large_size = fields.Integer(related="large_id.size")
    sized_id = fields.Many2one("bad.generated.large", domain="[('size', '>', 0)]")
