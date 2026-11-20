from odoo import api, fields, models


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


class Partner(models.Model):
    _inherit = "res.partner"

    socket_id = fields.Many2one("dependent.socket")
    parent_glow = fields.Char(related="parent_id.lamp_glow")
    parent_shade = fields.Char(related="parent_id.lamp_shade")
    socket_watts = fields.Integer(related="socket_id.lamp_watts")
    lit_id = fields.Many2one("res.partner", domain=[("lamp_glow", "=", "Warm")])
    shaded_id = fields.Many2one("res.partner", domain=[("lamp_shade", "=", "Linen")])

    @api.onchange("lamp_glow")
    def _onchange_lamp_glow(self):
        self.parent_glow = self.lamp_glow
