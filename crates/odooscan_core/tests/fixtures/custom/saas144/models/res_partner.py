from odoo import api, fields, models


class ResPartnerStamp(models.Model):
    _inherit = "res.partner"

    create_date = fields.Datetime(index=True)
    opened_on = fields.Datetime(string="Created on")
    stamped_on = fields.Datetime(string="Create Date")


class ResPartnerTotal(models.Model):
    _inherit = "res.partner"

    total = fields.Float(compute="_compute_total", store=True)

    @api.depends("child_ids.total")
    def _compute_total(self):
        for partner in self:
            partner.total = len(partner.child_ids)
