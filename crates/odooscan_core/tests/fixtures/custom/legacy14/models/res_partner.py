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

    initials = fields.Char(compute="_compute_initials")

    @api.depends("name")
    def _compute_initials(self):
        for partner in self:
            if partner.name:
                partner.initials = partner.name[:2]


class ResPartnerStamp(models.Model):
    _inherit = "res.partner"

    create_date = fields.Datetime(index=True)
    opened_on = fields.Datetime(string="Created on")
    stamped_on = fields.Datetime(string="Create Date")
    __last_update = fields.Datetime(string="Touched")
    touched_on = fields.Datetime(string="Last Modified on")
