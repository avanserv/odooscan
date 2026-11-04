from odoo import http
from odoo.http import request


class Uploads(http.Controller):
    @http.route("/public193/datas", type="http", auth="user", methods=["POST"])
    def encoded(self, data):
        request.env["ir.attachment"].sudo().create({"name": "note", "datas": data, "public": True})

    @http.route("/public193/raw", type="http", auth="user", methods=["POST"])
    def raw(self, upload):
        request.env["ir.attachment"].sudo().create({"name": "note", "raw": upload.read(), "public": True})

    @http.route("/public193/upload", type="http", auth="user", methods=["POST"])
    def upload(self, upload):
        return request.env["ir.attachment"].sudo()._upload_file(upload, {"name": "note", "public": True}).id
