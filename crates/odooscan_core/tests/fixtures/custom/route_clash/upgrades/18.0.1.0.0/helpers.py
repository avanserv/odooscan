from odoo import http


class CatalogScript(http.Controller):
    @http.route("/route_clash/catalog", auth="user")
    def script(self):
        return "script"
