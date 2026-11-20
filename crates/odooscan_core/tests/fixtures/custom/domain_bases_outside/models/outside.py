from odoo import fields, models


class ResPartner(models.Model):
    _inherit = "res.partner"

    def x_outside_lookup(self):
        self.env["domain.hidden"].search([("stamp", "=", "a")])
        self.env["domain.delegate"].search([("stamp", "=", "a")])
        self.env["domain.unread.inherits"].search([("stamp", "=", "a")])
        self.env["domain.whole"].search([("no_outside_whole", "=", "a")])
        return self.env["domain.whole"].search([("hidden_id.stamp", "=", "a")])


class ResPartnerPaths(models.Model):
    _inherit = "res.partner"

    x_hidden_id = fields.Many2one("domain.hidden")
    x_whole_id = fields.Many2one("domain.whole")
    x_hidden_stamp = fields.Char(related="x_hidden_id.stamp")
    x_outside_related = fields.Char(related="x_whole_id.no_outside_related")
