from odoo import api, fields, models


class OrphanHook(models.Model):
    _inherit = "orphan.hook"

    level = fields.Integer(compute="_compute_hook_level")
    stale = fields.Integer()

    # Reported: nothing names these, and no parent defines them.
    def _onchange_hook_partner(self):
        self.stale = 0

    @api.depends("name")
    def _compute_hook_stale(self):
        for hook in self:
            hook.stale = 1

    def _inverse_hook_stale(self):
        return None

    def _search_hook_stale(self, operator, value):
        return [("name", operator, value)]

    def onchange_hook_customer(self):
        return {}

    def _search_hook_spread(self, *args):
        return [("name", "=", args)]

    def onchange_hook_legacy(self, partner_id):
        return {"value": {"name": partner_id}}

    # Silent: the parent defines it.
    def _onchange_hook_parent(self):
        return None

    # Silent: a decorator other than the named ones registers the method.
    @api.onchange("name")
    def _onchange_hook_decorated(self):
        self.stale = 2

    @api.constrains("name")
    def _compute_hook_registered(self):
        return None

    # Silent: BaseModel defines it.
    @api.depends("name")
    def _compute_display_name(self):
        for hook in self:
            hook.display_name = hook.name

    # Silent: a field, a call, a getattr literal, an f-string prefix, a cron, a button, an RPC.
    @api.depends("name")
    def _compute_hook_level(self):
        for hook in self:
            hook.level = 1

    def _inverse_hook_called(self):
        return None

    def _search_hook_lookup(self, operator, value):
        return [("name", operator, value)]

    def _compute_hook_built_total(self):
        return None

    def _compute_hook_cron(self):
        return None

    def onchange_hook_button(self):
        return True

    def onchange_hook_rpc(self):
        return True

    # Silent: OD211 reports the super() call instead.
    def _compute_hook_super(self):
        return super()._compute_hook_super()

    # Silent: no field could run these with the arguments they require.
    def _compute_hook_with(self, record):
        return record

    def _inverse_hook_with(self, value):
        return value

    def _search_hook_text(self, html_fragment):
        return html_fragment

    def _search_hook_keyword(self, operator, value, *, strict):
        return strict

    def _onchange_hook_with(self, values):
        return values

    # Silent: the class binds the name twice, and only the later binding stays.
    def _compute_hook_twice(self):
        return None

    def _compute_hook_twice(self):
        return 1

    def _hook_helper(self, name):
        self._inverse_hook_called()
        getattr(self, "_search_hook_lookup")("=", name)
        return getattr(self, f"_compute_hook_built_{name}")()


class OrphanHookLine(models.Model):
    _name = "orphan.hook.line"
    _description = "Orphan hook line"

    # Silent: a new model's hook.
    def _compute_hook_line(self):
        return None
