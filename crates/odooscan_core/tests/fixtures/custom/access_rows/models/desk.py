from odoo import fields, models


class AccessDesk(models.Model):
    _name = "access.desk"
    _description = "Access desk"

    name = fields.Char()
