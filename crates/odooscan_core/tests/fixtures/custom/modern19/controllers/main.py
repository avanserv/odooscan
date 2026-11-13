from odoo import http
from odoo.http import request


class Lookup(http.Controller):
    @http.route("/modern19/lookup", type="jsonrpc", auth="user")
    def lookup(self, name):
        return request.env["res.partner"].sudo().search_count([("name", "==", name)])
