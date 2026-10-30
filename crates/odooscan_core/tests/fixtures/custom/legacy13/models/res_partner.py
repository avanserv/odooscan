from odoo import fields, models


class ResPartner(models.Model):
    _inherit = "res.partner"

    holder_id = fields.Many2one("res.partner", required=True, ondelete="set null")
    origin_ids = fields.Many2many("res.country")
    market_ids = fields.Many2many("res.country")
    type = fields.Selection(selection_add=[("site", "Site")])
    company_id = fields.Many2one("res.company")
    home_country_id = fields.Many2one("res.country", check_company=True)
    stage = fields.Char(tracking=True)
