from odoo import fields, models


class RrScoped(models.Model):
    _inherit = "rr.scoped"

    name = fields.Char(default="named here")
    added = fields.Char(required=True)
