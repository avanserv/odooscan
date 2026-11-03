from odoo import fields, http, models
from odoo.http import request
from odoo.osv import expression


class ResPartnerUpgrade(models.Model):
    _inherit = "res.partner"

    x_legacy_code = fields.Char(oldname="x_code")
    x_manager_id = fields.Many2one("res.users", track_visibility="onchange")

    def _x_as_manager(self, user):
        partners = self.sudo(user)
        partners.message_post(body="Moved", subtype="mail.mt_note")
        self.flush()
        self.recompute()
        self.invalidate_cache()
        self.refresh()
        self.user_has_groups("base.group_user")
        self.check_access_rights("read")
        self.check_access_rule("read")
        return self.env["res.partner"].read_group([], fields=["name"], groupby=["country_id"], lazy=False)

    def _x_domain(self):
        return expression.AND([[("name", "=", "x")]])


class UpgradeController(http.Controller):
    @http.route("/legacy12/upgrade", type="json", auth="user")
    def upgrade(self):
        return request.jsonrequest
