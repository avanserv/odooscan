from odoo import api, fields, http, models


class ResPartnerUpgrade(models.Model):
    _inherit = "res.partner"

    x_manager_id = fields.Many2one("res.users", track_visibility="onchange")

    def _x_check(self):
        self.check_access_rights("write")
        self.check_access_rights("read", raise_exception=False)
        self.check_access_rule("write")
        self.env["res.partner"].read_group([], fields=["name"], groupby=["country_id"])
        return self.user_has_groups("base.group_user")

    @api.model
    def _name_search(self, name, domain=None, operator="ilike", limit=None, order=None):
        return super()._name_search(name, domain, operator, limit=limit, order=order)

    @api.model
    def read_group(self, domain, fields, groupby, offset=0, limit=None, orderby=False, lazy=True):
        return super().read_group(domain, fields, groupby, offset=offset, limit=limit, orderby=orderby, lazy=lazy)

    def _x_schedule(self):
        cron = self.env["ir.cron"].sudo().create([{"name": "Sync", "numbercall": -1, "doall": True}])
        return cron.end_previous(doall=True)


class ResUsersUpgrade(models.Model):
    _inherit = "res.users"

    def user_has_groups(self, groups):
        return self.env.user.has_groups(groups)

    def _x_can_manage(self):
        return self.user_has_groups("base.group_system")


class UpgradeController(http.Controller):
    @http.route("/saas181/upgrade", type="json", auth="user")
    def upgrade(self):
        return {}
