from odoo import http


class Probe(http.Controller):
    @http.route("/route_auto/probe", auth="user")
    def probe(self):
        return "probe"
