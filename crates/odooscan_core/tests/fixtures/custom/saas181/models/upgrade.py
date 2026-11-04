from odoo import fields, http, models


class ResPartnerUpgrade(models.Model):
    _inherit = "res.partner"

    x_manager_id = fields.Many2one("res.users", track_visibility="onchange")

    def _x_check(self):
        self.check_access_rights("write")
        self.check_access_rule("write")
        self.env["res.partner"].read_group([], fields=["name"], groupby=["country_id"])
        return self.user_has_groups("base.group_user")


class UpgradeController(http.Controller):
    @http.route("/saas181/upgrade", type="json", auth="user")
    def upgrade(self):
        return {}
