from odoo import models


class ResPartner(models.Model):
    _inherit = "res.partner"

    def _gadget_rank(self):
        return 1
