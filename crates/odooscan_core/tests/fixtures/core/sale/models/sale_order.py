from odoo import fields, models


class SaleOrder(models.Model):
    _name = "sale.order"
    _description = "Sales order"

    order_ref = fields.Char()
