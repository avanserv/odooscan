from odoo import models


class AutoScopeThing(models.Model):
    _inherit = "auto.scope.thing"

    def _reads_auto_installed(self):
        self.search([("country_any_stamp", "=", "a")])
        self.search([("list_any_stamp", "=", "a")])
