from odoo import models


class ResPartnerUpgrade(models.Model):
    _inherit = "res.partner"

    def _x_note(self):
        self.message_post(body="Moved", subtype="mail.mt_note")
        self.message_post(body="Moved", subtype_xmlid="mail.mt_note")
