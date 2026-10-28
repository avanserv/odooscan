from odoo import fields, models


class ResPartner(models.Model):
    _inherit = "res.partner"

    country_currency_id = fields.Many2one("res.currency")
    balance = fields.Monetary(currency_field="country_currency_id")
    parent_balance = fields.Monetary(related="parent_id.balance")


class ResPartnerSearch(models.Model):
    _inherit = "res.partner"
    _rec_names_search = ["name", "nickname"]
