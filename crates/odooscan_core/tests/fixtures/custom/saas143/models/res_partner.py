from odoo import api, fields, models


class ResPartner(models.Model):
    _inherit = "res.partner"

    total = fields.Float(compute="_compute_total", store=True)

    @api.depends("child_ids.total")
    def _compute_total(self):
        for partner in self:
            partner.total = len(partner.child_ids)
