from odoo import http
from odoo.http import request
from odoo.tools.safe_eval import safe_eval

from .. import lib


class Configured(http.Controller):
    @http.route("/bad/configured", auth="user")
    def configured(self, term):
        lib.run_query("SELECT id FROM bad_order WHERE name = '%s'" % term)
        query = "SELECT id FROM bad_order WHERE name = '%s'" % lib.clean_term(term)
        request.env.cr.execute(query)
        form = lib.read_form()
        return safe_eval(form["expression"])
