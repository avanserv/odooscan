from odoo import http


class Pages(http.Controller):
    @http.route("/route_drop15/page", auth="user")
    def page(self):
        return "page"


class PagesExtended(Pages):
    def page(self):
        return super().page().upper()
