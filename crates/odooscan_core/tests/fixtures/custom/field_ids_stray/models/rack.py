from odoo import fields, models


class RackStray(models.Model):
    _inherit = "ghost.rack"

    stock = fields.Integer()
    shelf = fields.Char()
    bay = fields.Char()
