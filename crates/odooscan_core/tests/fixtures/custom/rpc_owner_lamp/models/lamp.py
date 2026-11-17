from odoo import fields, models


class RpcOwnerLamp(models.Model):
    _name = "rpc.owner.lamp"
    _description = "RPC owner lamp"

    name = fields.Char()
