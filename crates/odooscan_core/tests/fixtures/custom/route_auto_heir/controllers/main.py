from odoo import http


class SharedAgain(http.Controller):
    @http.route("/route_auto/shared", auth="user")
    def shared(self):
        return "shared again"

    @http.route("/route_auto/probe", auth="user")
    def probe(self):
        return "probe again"
