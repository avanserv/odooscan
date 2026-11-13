from odoo import http


class GadgetMap(http.Controller):
    @http.route("/gadget_map", auth="public")
    def gadget_map(self):
        return "<div/>"
