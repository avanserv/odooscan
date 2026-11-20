from odoo import fields, models


class Lamp(models.Model):
    _name = "dependent.lamp"
    _description = "Lamp"

    name = fields.Char()


class Socket(models.Model):
    _name = "dependent.socket"
    _description = "Socket"

    name = fields.Char()


class Ghost(models.Model):
    _inherit = "dependent.ghost"

    chill = fields.Char()


class Wiring(models.AbstractModel):
    _inherit = "dependent.wiring"

    gauge = fields.Integer()


class Partner(models.Model):
    _inherit = "res.partner"

    lamp_glow = fields.Char()
    lamp_shade = fields.Char()


class SocketWatts(models.Model):
    _inherit = "dependent.socket"

    lamp_watts = fields.Integer()
