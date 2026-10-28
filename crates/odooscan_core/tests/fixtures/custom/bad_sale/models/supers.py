from odoo import models


class SuperOrder(models.Model):
    _inherit = "bad.order"

    def _prepare_from_missing_module(self):
        return super()._prepare_from_missing_module()
