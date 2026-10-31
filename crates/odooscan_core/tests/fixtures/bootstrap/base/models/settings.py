from odoo import fields, models


class ResSettled(models.Model):
    _name = "res.settled"
    _description = "Holds what base_data.sql creates"

    name = fields.Char(required=True)
