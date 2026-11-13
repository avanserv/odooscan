from odoo import http
from odoo.exceptions import AccessDenied
from odoo.http import request


class Switch(http.Controller):
    @http.route("/session15/switch", auth="user", methods=["POST"])
    def switch(self, uid):
        request.uid = uid
        return "ok"

    @http.route("/session15/sign_in", auth="user", methods=["POST"])
    def sign_in(self, login, password):
        old_uid = request.uid
        try:
            return request.session.authenticate(request.db, login, password)
        except AccessDenied:
            request.uid = old_uid
            return False
