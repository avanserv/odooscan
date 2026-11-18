from odoo import http
from odoo.addons.route_widen_base.controllers.main import Routes


class R(Routes):
    @http.route(methods=["GET", "POST"])
    def kept(self):
        return "kept"
