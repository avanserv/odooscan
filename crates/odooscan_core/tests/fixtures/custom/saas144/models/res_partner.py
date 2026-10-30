from odoo import fields, models


class ResPartnerStamp(models.Model):
    _inherit = "res.partner"

    create_date = fields.Datetime(index=True)
    opened_on = fields.Datetime(string="Created on")
    stamped_on = fields.Datetime(string="Create Date")
