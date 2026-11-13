from odoo import http
from odoo.http import request


class Uploads(http.Controller):
    @http.route("/public_grant_domain/stored", type="http", auth="user", methods=["POST"])
    def stored(self, upload):
        request.env["ir.attachment"].create({"name": "note", "raw": upload.read(), "public": True})
