from odoo import api, fields, models


class ResPartner(models.Model):
    _inherit = "res.partner"

    country_currency_id = fields.Many2one("res.currency")
    balance = fields.Monetary(currency_field="country_currency_id")
    parent_balance = fields.Monetary(related="parent_id.balance")
    total = fields.Float(compute="_compute_total", store=True)
    has_access = fields.Boolean()

    @api.depends("child_ids.total")
    def _compute_total(self):
        for partner in self:
            partner.total = len(partner.child_ids)


class ResPartnerSearch(models.Model):
    _inherit = "res.partner"
    _rec_names_search = ["name", "nickname"]


class ResPartnerCreate(models.Model):
    _inherit = "res.partner"

    @api.model
    def create(self, vals):
        vals.setdefault("ref", vals.get("name"))
        return super().create(vals)
