from markupsafe import Markup

from odoo import http
from odoo.http import request


class Callers(http.Controller):
    @http.route("/bad/apply", auth="user", methods=["POST"])
    def apply(self, **post):
        request.env["bad.order"].sudo()._apply_values(post)
        request.env["bad.order"]._apply_values(post)
        request.env["bad.order"].sudo().with_user(request.env.user)._apply_values(post)
        return request.redirect("/bad/thanks")

    @http.route("/bad/find", auth="user")
    def find(self, term):
        self.term = term
        request.env.cr.execute("SELECT id FROM bad_order WHERE name = '%s'" % self.term)
        return request.env.cr.fetchall()

    @http.route("/bad/note", auth="user")
    def note(self, note):
        return Markup(request.env["bad.order"]._note_html(note))

    @http.route("/bad/label", auth="user")
    def label(self):
        return Markup(request.env["bad.order"].label("Order "))
