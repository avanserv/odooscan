from odoo import fields, models


class SoftThing(models.Model):
    _name = "soft.thing"
    _description = "Soft thing"

    code = fields.Char()
