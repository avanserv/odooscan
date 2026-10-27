from odoo import http
from odoo.http import request


class Portal(http.Controller):
    @http.route("/bad/order/<int:order_id>", auth="public", csrf=False, methods=["POST"])
    def order(self, order_id, **kw):
        return request.env["bad.order"].sudo().browse(order_id).name
