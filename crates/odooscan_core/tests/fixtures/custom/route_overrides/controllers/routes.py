from odoo import http
from odoo.addons.bad_sale.controllers.routes import Routes
from odoo.http import request


class RoutesOverride(Routes):
    @http.route("/bad/routes/rename/<int:order_id>")
    def rename(self, order_id):
        request.env["bad.order"].browse(order_id).write({"name": "Renamed again"})
        return request.redirect("/my")

    @http.route()
    def drop(self):
        request.env["bad.order"].search([("state", "=", "cancel")]).unlink()
        return request.redirect("/my")

    @http.route()
    def widget(self):
        return {"orders": request.env["bad.order"].sudo().search_count([])}

    @http.route()
    def greeting(self):
        return "Welcome %s" % request.env.user.name

    @http.route("/bad/routes/posted")
    def posted(self):
        request.env["bad.order"].create({"name": "Posted again"})
        return request.redirect("/my")

    @http.route()
    def confirm(self, order):
        return request.make_response("", [("X-Order", order.name)])
