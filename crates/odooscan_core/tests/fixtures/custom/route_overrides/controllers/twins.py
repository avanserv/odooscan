from odoo import http
from odoo.http import request


class Twins(http.Controller):
    @http.route("/route_overrides/twins/stock", auth="public")
    def stock(self):
        return str(request.env["bad.order"].sudo().search_count([]))

    @http.route("/route_overrides/twins/count", auth="user")
    def count(self):
        return str(request.env["bad.order"].sudo().search_count([]))


class Twins(http.Controller):
    @http.route("/route_overrides/twins/stock", auth="user")
    def stock(self):
        return str(request.env["bad.order"].sudo().search_count([]))

    @http.route("/route_overrides/twins/count", auth="public")
    def count(self):
        return str(request.env["bad.order"].sudo().search_count([]))


class TwinsExtended(Twins):
    @http.route()
    def stock(self):
        return super().stock()
