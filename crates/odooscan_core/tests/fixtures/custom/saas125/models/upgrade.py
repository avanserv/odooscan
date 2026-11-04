from odoo import SUPERUSER_ID, fields, models


class ResPartnerUpgrade(models.Model):
    _inherit = "res.partner"

    x_legacy_code = fields.Char(oldname="x_code")
    x_user_can_edit = fields.Boolean()

    def _x_as_manager(self, user):
        return self.sudo(user) | self.sudo(user.id) | self.sudo(1)

    def _x_as_admin(self):
        return self.sudo(self.env["res.users"].browse(2)) | self.sudo(self.env["res.users"].browse(2).id)

    def _x_as_caller(self, is_superuser, as_superuser):
        return self.sudo(self.env.su) | self.sudo(is_superuser) | self.sudo(self.x_user_can_edit) | self.sudo(as_superuser)

    def _x_as_root(self):
        return self.sudo(SUPERUSER_ID)
