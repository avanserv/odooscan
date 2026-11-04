from odoo import http
from odoo.exceptions import AccessError
from odoo.http import request
from odoo.tools import consteq


class Records(http.Controller):
    @http.route("/bad/records/inline/<int:order_id>", auth="user")
    def inline(self, order_id):
        return request.env["bad.order"].sudo().browse(order_id).name

    @http.route("/bad/records/local", auth="user")
    def local(self, order_id):
        order = request.env["bad.order"].sudo().browse(int(order_id))
        return order.read(["name"])

    @http.route("/bad/records/dict", auth="user", website=True)
    def rendered(self, order_id):
        order = request.env["bad.order"].browse(int(order_id)).sudo()
        return request.render("bad_sale.order", {"order": order})

    @http.route("/bad/records/rebound", auth="user", website=True)
    def rebound(self, order_id, **kw):
        order_id = int(order_id)
        values = {"page": "order"}
        values["order"] = request.env["bad.order"].sudo().browse(order_id).exists()
        return request.render("bad_sale.order", values)

    @http.route("/bad/records/unchecked", auth="user")
    def unchecked(self, order_id):
        order = request.env["bad.order"].sudo().browse(int(order_id))
        if not order.exists():
            raise AccessError("No such order")
        order.check_access("read")
        return order.name

    @http.route("/bad/records/token", auth="user")
    def token(self, order_id, access_token):
        order = request.env["bad.order"].sudo().browse(int(order_id))
        if not consteq(order.access_token or "", access_token):
            raise AccessError("Wrong token")
        return order.name

    @http.route("/bad/records/document", auth="user")
    def document(self, order_id, access_token):
        self._document_check_access("bad.order", int(order_id), access_token)
        return request.env["bad.order"].sudo().browse(int(order_id)).name

    @http.route("/bad/records/rights", auth="user")
    def rights(self, order_id):
        request.env["bad.order"].browse(int(order_id)).check_access("read")
        return request.env["bad.order"].sudo().browse(int(order_id)).name

    @http.route("/bad/records/owner", auth="user")
    def owner(self, order_id):
        order = request.env["bad.order"].sudo().browse(int(order_id))
        if order.partner_id != request.env.user.partner_id:
            raise AccessError("Not your order")
        return order.name

    @http.route("/bad/records/allowed", auth="user")
    def allowed(self, order_id):
        if int(order_id) not in request.env.user.partner_id.order_ids.ids:
            raise AccessError("Not your order")
        return request.env["bad.order"].sudo().browse(int(order_id)).name
