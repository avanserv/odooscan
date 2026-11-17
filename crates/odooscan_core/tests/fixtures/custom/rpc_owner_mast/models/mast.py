from odoo import fields, models


class RpcOwnerMast(models.Model):
    _name = "rpc.owner.mast"
    _description = "RPC owner mast"

    name = fields.Char()
