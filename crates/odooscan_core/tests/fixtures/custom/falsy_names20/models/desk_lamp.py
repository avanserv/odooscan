from odoo import fields, models


class ResPartner(models.Model):
    _name = False
    _inherit = ["res.country", "res.currency"]

    lamp_count = fields.Integer()


class DeskLamp(models.Model):
    _name = None
    _description = "Desk lamp"

    wattage = fields.Integer()


class DeskShade(models.Model):
    _name = ""
    _description = "Desk shade"

    color = fields.Char()
