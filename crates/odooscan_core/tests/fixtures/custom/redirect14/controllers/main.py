from odoo import http
from odoo.http import request


class Back(http.Controller):
    @http.route("/redirect14/back", auth="user")
    def back(self, **kw):
        return request.redirect(kw["next"])
