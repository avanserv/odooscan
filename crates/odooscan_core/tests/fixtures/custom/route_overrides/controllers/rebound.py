from odoo import http
from odoo.addons.bad_sale.controllers.routes import Routes
from odoo.http import request


class Routes(Routes):
    @http.route(methods=["GET", "POST"])
    def posted(self):
        return super().posted()

    @http.route(auth="user")
    def greeting(self):
        return super().greeting()


class RoutesLeaf(Routes):
    @http.route()
    def posted(self):
        request.env["bad.order"].create({"name": "Posted on GET"})
        return request.redirect("/my")

    @http.route()
    def greeting(self):
        return "Hi %s" % request.env.user.name
