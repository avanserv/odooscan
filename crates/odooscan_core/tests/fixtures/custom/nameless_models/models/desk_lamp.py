from odoo import fields, models


class DeskLamp(models.Model):
    _description = "Desk lamp"

    wattage = fields.Integer()


class DeskShade(models.Model):
    _description = "Desk shade"
    _inherit = []

    color = fields.Char()


class ResPartner(models.Model):
    _description = "Partner named after its class"

    lamp_count = fields.Integer()


class LampBase(models.Model):
    _register = False

    socket = fields.Char()
