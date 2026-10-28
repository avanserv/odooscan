from odoo import fields, models


class DomainOrder(models.Model):
    _inherit = "bad.order"

    company_partner_id = fields.Many2one("res.partner", domain="[('is_company', '=', True)]")
    child_partner_id = fields.Many2one("res.partner", domain=[("parent_id", "!=", False)])

    def action_domains(self):
        self.search([("stat", "=", "done")])
        self.env["res.partner"].sudo().search_count([("nmae", "=", "x")])
        self.search([("stat", "=", "done")] + self._extra())
        if "bad.optional" in self.env:
            self.env["res.partner"].search([("optional_id", "=", 1)])
        return self.search([("name", "=", "x")])
