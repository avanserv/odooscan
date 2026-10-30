from odoo import fields, models


class ResCompany(models.Model):
    _inherit = "res.company"

    home_country_id = fields.Many2one("res.country", check_company=True)


class ResPartner(models.Model):
    _inherit = "res.partner"

    home_country_id = fields.Many2one("res.country", check_company=True)
