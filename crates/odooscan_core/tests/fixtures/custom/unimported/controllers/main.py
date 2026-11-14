from odoo import http


class Main(http.Controller):
    @http.route("/unimported/main", auth="user")
    def main(self):
        return "main"
