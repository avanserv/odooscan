from odoo import http
from odoo.addons.bad_sale.controllers.routes import Routes


class RoutesMove(Routes):
    @http.route("/route_moved/new")
    def new(self):
        return "moved"


class Orders(http.Controller):
    @http.route("/bad/routes/new", auth="user")
    def orders(self):
        return "orders"


class OrdersMoved(http.Controller):
    @http.route("/route_moved/new", auth="user")
    def orders_moved(self):
        return "orders moved"
