from odoo import fields, models

KINDS = ["mall", "lim"]

for kind in KINDS:

    class Short(models.Model):
        _name = "s" + kind

        name = fields.Char()


class ShortHolder(models.Model):
    _name = "short.holder"

    name = fields.Char()
    order_id = fields.Many2one("side.order")
