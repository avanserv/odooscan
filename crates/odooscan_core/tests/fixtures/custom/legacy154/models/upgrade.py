from odoo import http, models
from odoo.http import request


class ResCompanyUpgrade(models.Model):
    _inherit = "res.company"

    def _x_as_manager(self, user):
        self.sudo(user).flush()
        self.env["res.company"].recompute()
        self.invalidate_cache()
        self.refresh()
        return self.sudo(self.env.su)


class UpgradeController(http.Controller):
    @http.route("/legacy154/upgrade", type="json", auth="user")
    def upgrade(self):
        return request.jsonrequest.get("params")
