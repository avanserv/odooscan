from odoo import SUPERUSER_ID, api, http
from odoo.http import request
from odoo.tools import consteq


class Session(http.Controller):
    @http.route("/bad/session/as_root", auth="public", methods=["POST"])
    def as_root(self):
        return request.env["bad.order"].with_user(SUPERUSER_ID).search_count([])

    @http.route("/bad/session/as_one", auth="public", methods=["POST"])
    def as_one(self):
        return request.env["bad.order"].with_user(1).search_count([])

    @http.route("/bad/session/as_root_env", auth="public", methods=["POST"])
    def as_root_env(self):
        return request.env(user=SUPERUSER_ID)["bad.order"].search_count([])

    @http.route("/bad/session/as_su", auth="public", methods=["POST"])
    def as_su(self):
        return request.env(su=True)["bad.order"].search_count([])

    @http.route("/bad/session/as_su_request", auth="public", methods=["POST"])
    def as_su_request(self):
        request.update_env(su=True)
        return request.env["bad.order"].search_count([])

    @http.route("/bad/session/as_root_environment", auth="public", methods=["POST"])
    def as_root_environment(self):
        env = api.Environment(request.env.cr, SUPERUSER_ID, {})
        return env["bad.order"].search_count([])

    @http.route("/bad/session/as_su_environment", auth="public", methods=["POST"])
    def as_su_environment(self):
        env = api.Environment(request.env.cr, request.env.uid, {}, su=True)
        return env["bad.order"].search_count([])

    @http.route("/bad/session/as_visitor", auth="public", methods=["POST"])
    def as_visitor(self):
        orders = request.env["bad.order"].with_user(request.env.user)
        count = request.env(su=False)["bad.order"].search_count([])
        request.update_env(context={"lang": "en_US"})
        return orders.sudo(False).search_count([]) + count

    @http.route("/bad/session/switch", auth="user", methods=["POST"])
    def switch(self, uid):
        request.update_env(user=uid)
        request.session.uid = uid
        return "ok"

    @http.route("/bad/session/sign_out", auth="user", methods=["POST"])
    def sign_out(self):
        request.session.uid = None
        return "ok"

    @http.route("/bad/session/restore", auth="none", methods=["POST"])
    def restore(self):
        request.update_env(user=request.session.uid)
        return "ok"

    @http.route("/bad/session/check", auth="user", methods=["POST"])
    def check(self, token, token_type):
        order = request.env["bad.order"].search([], limit=1)
        if token_type == "bearer" and consteq(token, order.access_token or ""):
            return "ok"
        if token != order.access_token:
            return "denied"
        return "ok"
