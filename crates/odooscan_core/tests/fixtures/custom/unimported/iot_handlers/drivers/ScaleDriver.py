from odoo import http


class ScaleController(http.Controller):
    @http.route("/unimported/scale", auth="none")
    def scale(self):
        return "scale"
