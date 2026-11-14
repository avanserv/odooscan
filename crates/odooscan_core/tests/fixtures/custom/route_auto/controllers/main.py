from odoo import http


class Shared(http.Controller):
    @http.route("/route_auto/shared", auth="user")
    def shared(self):
        return "shared"

    @http.route("/route_auto/side", auth="user")
    def side(self):
        return "side"
