from odoo import fields, models


class DeskLamp(models.Model):
    _description = "Desk lamp"

    wattage = fields.Integer()


class ResPartner(models.Model):
    _inherit = []

    lamp_count = fields.Integer()
