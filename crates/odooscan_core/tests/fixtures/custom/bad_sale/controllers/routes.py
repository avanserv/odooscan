import os

from odoo import SUPERUSER_ID, api, http
from odoo.fields import Command
from odoo.http import request


class Routes(http.Controller):
    @http.route("/bad/routes/rename/<int:order_id>", auth="user")
    def rename(self, order_id):
        request.env["bad.order"].browse(order_id).write({"name": "Renamed"})
        return request.redirect("/my")

    @http.route("/bad/routes/new", auth="user", methods=["GET", "POST"])
    def new(self):
        request.env["bad.order"].create({"name": "New"})
        return request.redirect("/my")

    @http.route("/bad/routes/drop", auth="user")
    def drop(self):
        orders = request.env["bad.order"].search([("state", "=", "draft")])
        orders.unlink()
        return request.redirect("/my")

    @http.route("/bad/routes/confirm/<model('bad.order'):order>", auth="user")
    def confirm(self, order):
        order.write({"state": "done"})
        return request.redirect("/my")

    @http.route("/bad/routes/stock", type="json", auth="public", cors="*")
    def stock(self):
        return {"available": True}

    @http.route("/bad/routes/greeting", auth="none")
    def greeting(self):
        return "Hello %s" % request.env.user.name

    @http.route("/bad/routes/ping", auth="none", methods=["POST"])
    def ping(self):
        request.env["bad.order"].create({"name": "Ping"})
        return "ok"

    @http.route("/bad/routes/posted", auth="user", methods=["POST"])
    def posted(self):
        request.env["bad.order"].create({"name": "Posted"})
        return request.redirect("/my")

    @http.route("/bad/routes/hook", auth="user", csrf=False)
    def hook(self):
        request.env["bad.order"].create({"name": "Hook"})
        return "ok"

    @http.route("/bad/routes/data", type="json", auth="user")
    def data(self):
        request.env["bad.order"].create({"name": "Data"})
        return {}

    @http.route()
    def inherited(self):
        request.env["bad.order"].create({"name": "Inherited"})
        return "ok"

    @http.route("/bad/routes/form", auth="user")
    def form(self):
        if request.httprequest.method == "POST":
            request.env["bad.order"].create({"name": "Form"})
        return request.render("bad_sale.form")

    @http.route("/bad/routes/token", auth="user")
    def token(self, csrf_token):
        if not request.validate_csrf(csrf_token):
            return request.redirect("/my")
        request.env["bad.order"].create({"name": "Token"})
        return request.redirect("/my")

    @http.route("/bad/routes/export", auth="user")
    def export(self, buffer, path):
        buffer.write(b"orders")
        os.unlink(path)
        return [Command.create({"name": "Line"})]

    @http.route("/bad/routes/widget", type="json", auth="public", cors="https://shop.example.com")
    def widget(self):
        return {}

    @http.route("/bad/routes/login", auth="none", methods=["POST"])
    def login(self, login, password):
        request.session.authenticate(request.db, {"login": login, "password": password, "type": "password"})
        return "Hello %s" % request.env.user.name

    @http.route("/bad/routes/as_admin", auth="none", methods=["POST"])
    def as_admin(self):
        request.update_env(user=SUPERUSER_ID)
        request.env["bad.order"].create({"name": "Admin"})
        return "ok"

    @http.route("/bad/routes/as_root", auth="none", methods=["POST"])
    def as_root(self):
        env = api.Environment(request.env.cr, SUPERUSER_ID, {})
        env["bad.order"].create({"name": "Root"})
        return "ok"

    @http.route("/bad/routes/anonymous", auth="none")
    def anonymous(self):
        request.update_env(user=None)
        return "Hello %s" % request.env.user.name

    @http.route("/bad/routes/stamp", auth="user")
    def stamp(self):
        new_env = request.env(user=request.session.uid)
        new_env["bad.order"].create({"name": "Stamp"})
        return request.redirect("/my")

    @http.route("/bad/routes/signed", auth="none", methods=["POST"])
    def signed(self):
        new_env = request.env(user=request.session.uid)
        new_env["bad.order"].create({"name": "Signed"})
        return "ok"
