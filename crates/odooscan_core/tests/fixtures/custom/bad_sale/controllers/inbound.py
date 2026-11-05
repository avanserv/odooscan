import base64
import json
import marshal
import os
import pickle
import shlex
import subprocess

import yaml
from lxml import etree

from odoo import http
from odoo.http import content_disposition, request
from odoo.tools import file_open


class Inbound(http.Controller):
    @http.route("/bad/inbound/print", auth="user")
    def print_label(self, name, copies="1"):
        subprocess.run(["lpr", "-#", copies, "-T", name], check=True)
        subprocess.run("lpr -T " + shlex.quote(name), shell=True, check=True)
        os.system("lpr -T %s" % name)
        return subprocess.check_output(f"lpq -P {copies}", shell=True)

    @http.route("/bad/inbound/restore", auth="user", type="json")
    def restore(self, saved, settings, code):
        json.loads(base64.b64decode(saved))
        yaml.load(settings, Loader=yaml.SafeLoader)
        yaml.safe_load(settings)
        pickle.loads(base64.b64decode(saved))
        marshal.loads(code)
        return yaml.load(settings, Loader=yaml.Loader)

    @http.route("/bad/inbound/feed", auth="user", methods=["POST"])
    def feed(self, feed, sheet):
        safe = etree.XMLParser(resolve_entities=False)
        etree.fromstring(feed.encode(), safe)
        etree.parse(file_open(sheet))
        etree.fromstring(feed.encode())
        return etree.fromstring(feed.encode(), etree.XMLParser(resolve_entities=True))

    @http.route(["/bad/inbound/export", "/bad/inbound/export/<model('res.partner'):partner>"], auth="user")
    def export(self, name, mimetype, partner=None):
        headers = [("Content-Disposition", content_disposition(name))]
        response = request.make_response("", headers)
        if partner:
            response.headers["X-Partner"] = partner
        response.headers["Content-Type"] = mimetype
        return request.make_response("", [("Content-Disposition", f"attachment; filename={name}")])
