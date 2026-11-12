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
