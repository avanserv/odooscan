from odoo import models


class ResPartner(models.Model):
    _inherit = "res.partner"

    def x_outside_lookup(self):
        self.env["domain.hidden"].search([("stamp", "=", "a")])
        self.env["domain.delegate"].search([("stamp", "=", "a")])
        self.env["domain.unread.inherits"].search([("stamp", "=", "a")])
        self.env["domain.whole"].search([("no_outside_whole", "=", "a")])
        return self.env["domain.whole"].search([("hidden_id.stamp", "=", "a")])
