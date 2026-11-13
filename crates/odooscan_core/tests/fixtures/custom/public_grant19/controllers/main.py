from odoo import http
from odoo.http import request


class Uploads(http.Controller):
    @http.route("/public_grant19/stored", type="http", auth="public", methods=["POST"])
    def stored(self, upload):
        request.env["ir.attachment"].create({"name": "note", "raw": upload.read(), "public": True})
