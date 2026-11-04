from odoo import fields, models


class PortPartner(models.Model):
    _inherit = "res.partner"

    port_score = fields.Integer(group_operator="max")

    def _port_is_user(self):
        return self.user_has_groups("base.group_user")

    def _port_save(self):
        self.env.cr.commit()


class PortGone(models.Model):
    _inherit = "port.gone"
