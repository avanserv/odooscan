from odoo import api, models


class ResPartnerUpgrade(models.Model):
    _inherit = "res.partner"

    @api.onchange("parent_id")
    def _onchange_x_parent(self):
        if not self.parent_id:
            return {}
        return {"domain": {"child_ids": [("parent_id", "=", self.parent_id.id)]}}

    def _x_child_domain(self):
        return {"domain": [("parent_id", "=", self.id)]}
