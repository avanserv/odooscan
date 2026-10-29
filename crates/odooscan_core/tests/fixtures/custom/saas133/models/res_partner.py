from odoo import api, fields, models


class ResPartner(models.Model):
    _inherit = "res.partner"

    monogram = fields.Char(compute="_compute_monogram")

    @api.depends("name")
    def _compute_monogram(self):
        for partner in self:
            if partner.name:
                partner.monogram = partner.name[0]
