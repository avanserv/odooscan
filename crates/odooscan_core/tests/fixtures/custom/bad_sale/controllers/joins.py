from odoo import http
from odoo.http import request
from odoo.osv.expression import AND


class Joins(http.Controller):
    def _match_key(self):
        return "ref"

    @http.route("/bad/joins/partners", auth="user", type="json")
    def partners(self, extra, field, term):
        partners = request.env["res.partner"].sudo()
        active = [("active", "=", True)]
        mine = [("is_company", "=", False)]
        partners.search_count(AND([active, mine, extra]))
        op = "ilike"
        partners.search_count(AND(["name", op, extra]))
        email_field = "email"
        partners.search_read([], ["name", email_field, field])
        key = self._match_key()
        return partners.search_count([(f"parent_id.{key}", "=", term)])
