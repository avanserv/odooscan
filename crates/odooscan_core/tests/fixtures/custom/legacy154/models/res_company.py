from odoo import models


class ResCompany(models.Model):
    _inherit = "res.company"

    def _neutralize(self):
        return super()._neutralize()

    def _get_access_action(self, access_uid=None, force_website=False):
        return super()._get_access_action(access_uid=access_uid, force_website=force_website)

    def _neutralized(self):
        return super()._neutralized()
