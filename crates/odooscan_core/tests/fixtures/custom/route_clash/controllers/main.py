from odoo import http
from odoo.addons.bad_sale.controllers.routes import Routes

PATH = "/bad/routes/stock"


def logged(func):
    return func


class NewOrders(http.Controller):
    @http.route("/bad/routes/new", auth="user")
    def create_order(self):
        return "ok"


class Catalog(http.Controller):
    @http.route("/route_clash/catalog", auth="user")
    def catalog(self):
        return "catalog"


class CatalogPage(http.Controller):
    @http.route("/route_clash/catalog", auth="user")
    def page(self):
        return "page"


class RoutesDrop(Routes):
    def drop(self):
        return "kept"


class RoutesStock(Routes):
    @http.route("/bad/routes/stock")
    def stock(self):
        return {"available": False}


class GreetingLeft(Routes):
    @http.route("/route_clash/greet")
    def greeting(self):
        return "left"


class GreetingRight(Routes):
    @http.route("/route_clash/greet")
    def greeting(self):
        return "right"


class Purge(http.Controller):
    @http.route("/bad/routes/posted", auth="user", methods=["DELETE"])
    def purge(self):
        return "ok"


class PingMixin(object):
    @http.route("/bad/routes/ping", auth="user")
    def ping(self):
        return "pong"


class RoutesWrapped(Routes):
    @logged
    def confirm(self, order):
        return "ok"


class Dynamic(http.Controller):
    @http.route(PATH, auth="user")
    def dynamic(self):
        return "ok"
