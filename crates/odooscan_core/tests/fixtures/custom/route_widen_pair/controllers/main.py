from odoo import http
from odoo.addons.route_widen_base.controllers.main import Other, Routes


class R(Routes):
    @http.route(methods=["GET", "POST"])
    def pair(self):
        return "pair"


class O(Other):
    @http.route(methods=["GET", "POST"])
    def other_pair(self):
        return "other pair"
