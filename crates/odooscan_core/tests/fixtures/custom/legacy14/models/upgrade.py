from odoo import fields, models


class ResPartnerUpgrade(models.Model):
    _inherit = "res.partner"

    x_manager_id = fields.Many2one("res.users", track_visibility="onchange")

    def _x_reload(self):
        self.refresh()
