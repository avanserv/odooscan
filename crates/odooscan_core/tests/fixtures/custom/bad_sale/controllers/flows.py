from markupsafe import Markup, escape

from odoo import http
from odoo.http import request
from odoo.tools import SQL
from odoo.tools.safe_eval import safe_eval


class Flows(http.Controller):
    @http.route("/bad/search", auth="user")
    def search(self, term, **kw):
        where = "name ilike '%%%s%%'" % term
        query = "SELECT id FROM bad_order WHERE " + where
        request.env.cr.execute(query)
        request.env.cr.execute("SELECT id FROM bad_order WHERE name = '%s'" % kw.get("name"))
        request.env.cr.execute("SELECT id FROM bad_order WHERE name = %s", (term,))
        request.env.cr.execute(SQL("SELECT id FROM bad_order WHERE name = %s", term))
        by_id = "SELECT id FROM bad_order WHERE id = %s" % int(kw["id"])
        request.env.cr.execute(by_id)
        return request.env.cr.fetchall()

    @http.route("/bad/compute", auth="user", type="json")
    def compute(self):
        data = request.get_json_data()
        return safe_eval(data["expression"])

    @http.route("/bad/preview", auth="public")
    def preview(self, **post):
        title = escape(post.get("title"))
        body = Markup("<p>%s</p>") % post.get("body")
        note = Markup(f"<p>{post.get('note')}</p>")
        return title + body + note

    @http.route("/bad/register", auth="public", methods=["POST"])
    def register(self, **post):
        Orders = request.env["bad.order"].sudo()
        values = dict(post)
        Orders.create(values)
        allowed = {key: post[key] for key in post if key in ("name", "date")}
        Orders.create(allowed)
        Orders.create({"name": post.get("name")})
        return request.redirect("/bad/thanks")

    @http.route("/bad/profile", auth="user", methods=["POST"])
    def profile(self, **post):
        if any(key not in ("name", "phone") for key in post):
            raise ValueError("unexpected field")
        request.env.user.sudo().write(post)

    @http.route("/bad/lookup", auth="user")
    def lookup(self, term):
        return request.env["bad.order"]._search_by(term)

    @http.route("/bad/banner", auth="user")
    def banner(self):
        return Markup(self._banner_text())

    def _banner_text(self):
        return request.params.get("banner")
