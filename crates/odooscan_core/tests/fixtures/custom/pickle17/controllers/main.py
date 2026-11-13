import base64

from odoo import http
from odoo.tools import pickle


class Restore(http.Controller):
    @http.route("/pickle17/restore", auth="user", type="json")
    def restore(self, saved):
        return pickle.loads(base64.b64decode(saved))
