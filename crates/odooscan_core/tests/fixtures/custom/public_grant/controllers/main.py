from odoo import http
from odoo.http import request


class Uploads(http.Controller):
    @http.route("/public_grant/stored", type="http", auth="user", methods=["POST"])
    def stored(self, upload):
        request.env["ir.attachment"].create({"name": "note", "raw": upload.read(), "public": True})

    @http.route("/public_grant/upload", type="http", auth="user", methods=["POST"])
    def upload(self, upload):
        return request.env["ir.attachment"]._upload_file(upload, {"name": "note", "public": True}).id

    @http.route("/public_grant/sudo", type="http", auth="user", methods=["POST"])
    def stored_as_superuser(self, upload):
        request.env["ir.attachment"].sudo().create({"name": "note", "raw": upload.read(), "public": True})

    @http.route("/public_grant/private", type="http", auth="user", methods=["POST"])
    def private(self, upload):
        request.env["ir.attachment"].create({"name": "note", "raw": upload.read()})
