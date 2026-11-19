from odoo import fields, models


class LampEarly(models.AbstractModel):
    _inherit = "late.lamp"

    glow = fields.Char()


class Bulb(models.AbstractModel):
    _name = "late.bulb"
    _inherit = ["late.lamp"]
    _description = "Bulb"


class Lamp(models.AbstractModel):
    _name = "late.lamp"
    _description = "Lamp"

    shade = fields.Char()


class LampLate(models.AbstractModel):
    _inherit = "late.lamp"

    switch = fields.Char()


class Stand(models.AbstractModel):
    _name = "late.stand"
    _description = "Stand"


class ShadeEarly(models.AbstractModel):
    _inherit = "late.shade"

    tint = fields.Char()


class Shade(models.AbstractModel):
    _name = "late.shade"
    _description = "Shade"


class ShadeLate(models.AbstractModel):
    _inherit = "late.shade"

    opacity = fields.Char()
