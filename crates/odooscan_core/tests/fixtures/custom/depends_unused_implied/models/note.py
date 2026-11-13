from odoo import models


class ResPartner(models.Model):
    _inherit = "res.partner"

    def _implied_threads(self):
        return self.env["mail.thread"]
