from odoo import http
from odoo.http import request


class Back(http.Controller):
    @http.route("/redirect15/back", auth="user")
    def back(self, **kw):
        return request.redirect(kw["next"])

    @http.route("/redirect15/query", auth="user")
    def query(self, **kw):
        return request.redirect_query(kw["next"], {})

    @http.route("/redirect15/shop", auth="user")
    def shop(self, **kw):
        return request.redirect(f"/shop/{kw['next']}")
