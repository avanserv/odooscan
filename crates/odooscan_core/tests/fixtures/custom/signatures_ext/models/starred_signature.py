from odoo import models


class StarredSignature(models.Model):
    _inherit = "starred.signature"

    # Silent: the star import may rebind the plain base, which the scan then cannot read.
    def _prepare(self, order):
        return super()._prepare(order, False)
