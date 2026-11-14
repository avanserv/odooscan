from odoo import http

from .main import Catalog


class CatalogAgain(Catalog):
    @http.route("/route_clash/catalog", auth="user")
    def again(self):
        return "again"

    def catalog(self):
        return "catalog again"
