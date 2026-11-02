from odoo import api, fields, http, models


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


class IrHttp(models.AbstractModel):
    _inherit = "ir.http"

    @classmethod
    def _handle_exception(cls, exception):
        http.request.env.cr.rollback()
        return exception


class ResPartnerNames(models.Model):
    _inherit = "res.partner"

    def name_get(self):
        return super().name_get()

    def _name_gone(self):
        return super()._name_gone()


class ResPartnerDunders(models.Model):
    _inherit = "res.partner"

    def __init__(self, env, ids, prefetch_ids):
        super().__init__(env, ids, prefetch_ids)

    def __getitem__(self, key):
        return super().__getitem__(key)

    def __setattr__(self, name, value):
        super().__setattr__(name, value)

    def __nope__(self):
        return super().__nope__()
