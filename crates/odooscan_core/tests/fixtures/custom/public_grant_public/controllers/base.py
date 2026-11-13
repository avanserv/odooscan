from odoo import http


class UploadsBase(http.Controller):
    @http.route("/public_grant_public/override", type="http", auth="public", methods=["POST"])
    def override(self, upload):
        pass
