from odoo import fields, models


class DeskBase(models.Model):
    _name = "desk.base"
    _description = "Desk base"

    height = fields.Integer()


class DeskLamp(DeskBase):
    wattage = fields.Integer()
