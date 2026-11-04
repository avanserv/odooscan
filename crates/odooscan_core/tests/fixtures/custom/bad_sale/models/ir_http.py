from odoo import models
from odoo.http import request


class IrHttp(models.AbstractModel):
    _inherit = "ir.http"

    @classmethod
    def _auth_method_kiosk(cls):
        user = request.env["res.users"].sudo().search([("login", "=", "kiosk")], limit=1)
        request.update_env(user=user.id)
