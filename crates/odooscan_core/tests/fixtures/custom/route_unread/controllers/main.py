from odoo import http
from odoo.addons.bad_sale.controllers.routes import Routes

from .mixins import UnreadMixin


class RoutesMixed(UnreadMixin, Routes):
    def drop(self):
        return super().drop()


class AutoCopy(http.Controller):
    @http.route("/route_auto/side", auth="user")
    def side(self):
        return "side copy"
