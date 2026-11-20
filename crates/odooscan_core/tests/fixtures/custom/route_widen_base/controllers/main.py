from odoo import http


class Routes(http.Controller):
    @http.route("/route_widen/x", methods=["POST"])
    def x(self):
        return "x"

    @http.route("/route_widen/kept", methods=["POST"])
    def kept(self):
        return "kept"

    @http.route("/route_widen/put", methods=["POST"])
    def put(self):
        return "put"

    @http.route("/route_widen/moved", methods=["POST"])
    def moved(self):
        return "moved"

    @http.route("/route_widen/pair", methods=["POST"])
    def pair(self):
        return "pair"


class Other(http.Controller):
    @http.route("/route_widen/x", methods=["GET"])
    def other_x(self):
        return "other x"

    @http.route("/route_widen/kept", methods=["GET"])
    def other_kept(self):
        return "other kept"

    @http.route("/route_widen/put", methods=["GET"])
    def other_put(self):
        return "other put"

    @http.route("/route_widen/moved", methods=["GET"])
    def other_moved(self):
        return "other moved"

    @http.route("/route_widen/pair", methods=["GET"])
    def other_pair(self):
        return "other pair"
