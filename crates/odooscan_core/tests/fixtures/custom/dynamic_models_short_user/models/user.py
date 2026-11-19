from odoo import fields, models


class ShortUser(models.Model):
    _name = "short.user"

    holder_id = fields.Many2one("short.holder")
    order_id = fields.Many2one("side.order")
    order_name = fields.Char(related="order_id.name")
