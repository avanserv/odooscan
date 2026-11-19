from odoo import fields, models


class Lamp(models.Model):
    _inherit = "dependent.lamp"

    glow = fields.Char()


class Ghost(models.Model):
    _inherit = "dependent.ghost"

    haunt = fields.Char()


class Wiring(models.AbstractModel):
    _name = "dependent.wiring"
    _description = "Wiring"

    socket_id = fields.Many2one("dependent.socket")
    partner_id = fields.Many2one("res.partner")
