from odoo import http


class Lent(http.Controller):
    @http.route("/unimported/lent", auth="user")
    def lent(self):
        return "lent"
