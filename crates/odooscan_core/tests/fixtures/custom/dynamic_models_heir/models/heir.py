from odoo import fields, models


class HeirHolder(models.Model):
    _name = "heir.holder"

    large_id = fields.Many2one("bad.generated.large")
    order_id = fields.Many2one("side.order")
