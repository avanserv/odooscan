from odoo import api, models


class ResPartnerUpgrade(models.Model):
    _inherit = "res.partner"

    def _x_sync(self):
        self.flush()
        self.env["res.partner"].recompute()
        self.invalidate_cache()
        self.refresh()
        self.env.cr.flush()
        return self.user_has_groups("base.group_user")

    @api.onchange("parent_id")
    def _onchange_x_parent(self):
        return {"domain": {"child_ids": [("parent_id", "=", self.parent_id.id)]}}

    @api.model
    def _name_search(self, name, domain=None, operator="ilike", limit=None, order=None):
        return super()._name_search(name, domain, operator, limit=limit, order=order)

    def _x_schedule(self):
        return self.env["ir.cron"].sudo().create([{"name": "Sync", "numbercall": -1, "doall": True}])


class ResUsersUpgrade(models.Model):
    _inherit = "res.users"

    def refresh(self):
        return self.read(["name"])

    def _x_sync(self):
        return self.refresh() | self.sudo().refresh()
