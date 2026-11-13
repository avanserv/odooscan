from odoo import http
from odoo.http import request


class Uploads(http.Controller):
    @http.route("/public_grant_public/signed_in", type="http", auth="user", methods=["POST"])
    def signed_in(self, upload):
        request.env["ir.attachment"].create({"name": "note", "raw": upload.read(), "public": True})

    @http.route("/public_grant_public/visitor", type="http", auth="public", methods=["POST"])
    def visitor(self, upload):
        request.env["ir.attachment"].create({"name": "note", "raw": upload.read(), "public": True})
