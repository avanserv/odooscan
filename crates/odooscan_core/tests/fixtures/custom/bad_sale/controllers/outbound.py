import io
import os
from urllib.parse import urlencode

import requests
import werkzeug
from werkzeug.utils import secure_filename

from odoo import http
from odoo.http import request
from odoo.modules.module import Manifest
from odoo.tools import file_open

DOCS = "/srv/bad_sale/docs"


class Outbound(http.Controller):
    @http.route("/bad/outbound/next", auth="user")
    def next_page(self, **kw):
        request.redirect(kw["next"])
        request.redirect(kw["next"], local=False)
        werkzeug.utils.redirect("/bad/thanks?" + urlencode({"next": kw["next"]}))
        werkzeug.utils.redirect("/shop/%s" % kw["next"])
        return werkzeug.utils.redirect(kw["next"])

    @http.route("/bad/outbound/fetch", auth="user", type="json")
    def fetch(self, url, product):
        requests.get(f"https://catalog.example.com/products/{product}", timeout=10)
        return requests.get(url, timeout=10).text

    @http.route("/bad/outbound/document", auth="user")
    def document(self, name):
        open(os.path.join(DOCS, os.path.basename(name)), "rb")
        open(DOCS + "/" + secure_filename(name), "rb")
        http.send_file(io.BytesIO(name.encode()), filename="document.txt")
        # file_open() opens files under the addons paths only.
        file_open(name)
        open(DOCS + "/" + name, "rb")
        os.path.join(DOCS, name)
        return http.send_file(name)

    @http.route("/bad/outbound/upload", auth="user", methods=["POST"])
    def upload(self, **kw):
        # werkzeug names each uploaded file by its form field, which the code picks here.
        for upload in request.httprequest.files.getlist("manual"):
            open(getattr(upload, "name"), "rb")
            open(upload.filename, "rb")
            request.env["bad.order"]._store_upload(upload)
        for upload in request.httprequest.files.values():
            open(upload.name, "rb")

    @http.route("/bad/outbound/page", auth="user")
    def page(self, **kw):
        page = os.path.normpath(os.path.join(DOCS, kw["page"]))
        if page.startswith(DOCS):
            open(page, "rb")
        unchecked = os.path.normpath(os.path.join(DOCS, kw["page"]))
        open(unchecked, "rb")
        manifest = Manifest.for_addon(kw["addon"])
        return http.send_file(os.path.join(manifest.path, "README.rst"))
