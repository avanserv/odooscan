from odoo import fields, models


class ButtonVisit(models.AbstractModel):
    _name = "button.visit"
    _description = "Button visit"

    name = fields.Char()
