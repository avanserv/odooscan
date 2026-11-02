from odoo import fields, models


class AutoScopeThing(models.Model):
    _inherit = "auto.scope.thing"

    peer_id = fields.Many2one("auto.scope.thing")
    client_id = fields.Many2one("auto.scope.thing", domain=[("next_stamp", "=", "a")])
    checked_id = fields.Many2one("auto.scope.thing", domain="[('next_stamp', '=', 'a')]")

    def _auto_installed(self):
        self.search([("both_stamp", "=", "a")])
        self.search([("next_stamp", "=", "a")])
        self.search([("drag_stamp", "=", "a")])
        self.search([("country_old_stamp", "=", "a")])

    def _out_of_reach(self):
        self.search([("far_stamp", "=", "a")])
        self.search([("country_stamp", "=", "a")])
        self.search([("country_any_stamp", "=", "a")])
        self.search([("both_next_id.no_past_both", "=", "a")])
