from odoo import http
from odoo.exceptions import UserError
from odoo.http import request


class PartnerImport(http.Controller):

    @http.route("/x_partner/import", type="json", auth="user")
    def import_partners(self):
        try:
            request.env["res.partner"].x_import_pending()
        except UserError:
            request.env.cr.rollback()
            return {"status": "failed"}
        return {"status": "done"}

    @http.route("/x_partner/touch", type="json", auth="user")
    def touch_partners(self):
        request.env.cr.execute("UPDATE res_partner SET write_date = now() WHERE active")
        return {"status": "done"}
