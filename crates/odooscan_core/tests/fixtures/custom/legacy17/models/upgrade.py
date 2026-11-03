from odoo import models


class ResPartnerUpgrade(models.Model):
    _inherit = "res.partner"

    def _x_sync(self):
        self.flush()
        self.env["res.partner"].recompute()
        self.invalidate_cache()
        self.refresh()
        self.env.cr.flush()
        return self.user_has_groups("base.group_user")


class ResUsersUpgrade(models.Model):
    _inherit = "res.users"

    def refresh(self):
        return self.read(["name"])

    def _x_sync(self):
        return self.refresh()
