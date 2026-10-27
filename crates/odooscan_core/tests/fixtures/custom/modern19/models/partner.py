from odoo import models


class ResPartner(models.Model):
    _inherit = "res.partner"
    _sql_constraints = [("name_uniq", "unique(name)", "Name must be unique")]

    def action_reset(self):
        self._cr.execute("UPDATE res_partner SET name = NULL WHERE id IN %s", (tuple(self.ids),))
