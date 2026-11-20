from odoo import http
from odoo.addons.route_widen_base.controllers.main import Routes


class R(Routes):
    @http.route(methods=["GET", "POST"])
    def x(self):
        return "x"

    @http.route()
    def kept(self):
        return "kept"

    @http.route(methods=["POST", "PUT"])
    def put(self):
        return "put"

    @http.route("/route_widen/elsewhere", methods=["GET", "POST"])
    def moved(self):
        return "moved"
