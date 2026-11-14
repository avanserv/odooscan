from odoo import http


class Page(http.Controller):
    @http.route("/route_rank_old/page", auth="user")
    def page(self):
        return "page"
