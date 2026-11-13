from odoo import http


class StockProbe(http.Controller):
    @http.route("/bad/routes/stock", auth="user")
    def probe(self):
        return "probe"
