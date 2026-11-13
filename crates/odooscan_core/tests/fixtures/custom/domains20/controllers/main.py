from odoo import http
from odoo.fields import Domain
from odoo.http import request


class Directory(http.Controller):
    @http.route("/domains20/count", type="jsonrpc", auth="user")
    def count(self, filters, name):
        active = Domain("active", "=", True)
        named = request.env["res.partner"].sudo().search_count(active & Domain("name", "ilike", name))
        filtered = request.env["res.partner"].sudo().search_count(active & Domain(filters))
        return {"named": named, "filtered": filtered}

    @http.route("/domains20/reachable", type="jsonrpc", auth="user")
    def reachable(self, mode):
        partners = request.env["res.partner"].sudo()
        listed = partners.search_count([("user_id", "access", mode)])
        built = partners.search_count(Domain("user_id", "access", mode))
        return {"listed": listed, "built": built}
