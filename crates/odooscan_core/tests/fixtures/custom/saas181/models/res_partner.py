from odoo import api, models


class ResPartner(models.Model):
    _inherit = "res.partner"

    @api.model
    def create(self, vals):
        vals.setdefault("ref", vals.get("name"))
        return super().create(vals)
