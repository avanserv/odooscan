from odoo import api, models
from odoo.tools.safe_eval import safe_eval


class SaleOrderFlows(models.Model):
    _inherit = "bad.order"

    def rename_all(self, name, vals):
        self.env.cr.execute(f"UPDATE bad_order SET name = '{name}'")
        self.sudo().write(vals)

    def _rename_one(self, name):
        self.env.cr.execute(f"UPDATE bad_order SET name = '{name}' WHERE id = {self.id}")

    @api.private
    def rename_private(self, name):
        self.env.cr.execute(f"UPDATE bad_order SET name = '{name}'")

    def apply_mode(self, mode):
        assert mode in ("upper", "lower")
        return safe_eval(mode)

    def _search_by(self, term):
        self.env.cr.execute("SELECT id FROM bad_order WHERE name = %s", [term])
        return self.env.cr.fetchall()


class SaleOrderSearch(models.Model):
    _inherit = "bad.order"

    def _search_by(self, term):
        query = "SELECT id FROM bad_order WHERE name = '%s'" % term
        self.env.cr.execute(query)
        return super()._search_by(term)
