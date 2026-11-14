from odoo import http


class Side(http.Controller):
    @http.route("/route_auto/side", auth="user")
    def side(self):
        return "side again"
