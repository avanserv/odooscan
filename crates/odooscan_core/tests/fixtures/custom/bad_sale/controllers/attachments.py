import base64

from odoo import http
from odoo.http import request


class Attachments(http.Controller):
    @http.route("/bad/attachments/inline", type="http", auth="user", methods=["POST"])
    def inline(self, upload):
        request.env["ir.attachment"].sudo().create({"name": "note", "raw": upload.read(), "public": True})

    @http.route("/bad/attachments/local", type="http", auth="user", methods=["POST"])
    def local(self, upload):
        Attachment = request.env["ir.attachment"].sudo()
        vals = {"name": "note", "raw": upload.read(), "public": True}
        return Attachment.create(vals).id

    @http.route("/bad/attachments/item", type="http", auth="user", methods=["POST"])
    def item(self, upload):
        vals = {"name": "note"}
        vals["db_datas"] = upload.read()
        vals["public"] = True
        request.env["ir.attachment"].sudo().create(vals)

    @http.route("/bad/attachments/dict", type="http", auth="user", methods=["POST"])
    def built(self, upload):
        vals = dict(name="note", raw=upload.read(), public=True)
        request.env["ir.attachment"].sudo().create(vals)

    @http.route("/bad/attachments/list", type="http", auth="user", methods=["POST"])
    def listed(self, first, second):
        request.env["ir.attachment"].sudo().create(
            [
                {"name": "first", "raw": first.read(), "public": True},
                {"name": "second", "raw": second.read(), "public": True},
            ]
        )

    @http.route("/bad/attachments/spread", type="http", auth="user", methods=["POST"])
    def spread(self, **post):
        request.env["ir.attachment"].sudo().create({**post, "public": True})

    @http.route("/bad/attachments/datas", type="json", auth="user")
    def encoded(self, data):
        request.env["ir.attachment"].sudo().create({"name": "note", "datas": data, "public": True})

    @http.route("/bad/attachments/rights", type="http", auth="user", methods=["POST"])
    def rights(self, upload):
        request.env["ir.attachment"].create({"name": "note", "raw": upload.read(), "public": True})

    @http.route("/bad/attachments/user", type="http", auth="user", methods=["POST"])
    def user(self, upload):
        Attachment = request.env["ir.attachment"]
        Attachment.sudo().with_user(request.env.user).create({"name": "note", "raw": upload.read(), "public": True})

    @http.route("/bad/attachments/other", type="http", auth="user", methods=["POST"])
    def other(self, upload):
        request.env["bad.order"].sudo().create({"name": "note", "raw": upload.read(), "public": True})

    @http.route("/bad/attachments/computed", type="http", auth="user", methods=["POST"])
    def computed(self, upload, shared=False):
        vals = {"name": "note", "raw": upload.read(), "public": bool(shared)}
        request.env["ir.attachment"].sudo().create(vals)

    @http.route("/bad/attachments/private", type="http", auth="user", methods=["POST"])
    def private(self, upload):
        request.env["ir.attachment"].sudo().create({"name": "note", "raw": upload.read()})

    @http.route("/bad/attachments/named", type="http", auth="user", methods=["POST"])
    def named(self, name):
        logo = base64.b64decode(b"aGVsbG8=")
        request.env["ir.attachment"].sudo().create({"name": name, "raw": logo, "public": True})
