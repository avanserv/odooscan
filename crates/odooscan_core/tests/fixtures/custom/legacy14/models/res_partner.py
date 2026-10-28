from odoo import api, fields, models


class ResPartner(models.Model):
    _inherit = "res.partner"

    type = fields.Selection(selection_add=[("site", "Site")])
    country_currency_id = fields.Many2one("res.currency")
    balance = fields.Monetary(currency_field="country_currency_id")
    parent_balance = fields.Monetary(related="parent_id.balance")
    stage = fields.Char(tracking=True)
    total = fields.Float(compute="_compute_total", store=True)

    @api.depends("child_ids.total")
    def _compute_total(self):
        for partner in self:
            partner.total = len(partner.child_ids)
