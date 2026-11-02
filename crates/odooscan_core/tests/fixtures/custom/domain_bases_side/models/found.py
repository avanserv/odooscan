from odoo import fields, models


class DomainFound(models.Model):
    _inherit = "domain.found"

    side_stamp = fields.Char()
