from odoo import fields, models


class MailThread(models.AbstractModel):
    _name = "mail.thread"

    message_ids = fields.One2many("res.partner", "parent_id")

    def _valid_field_parameter(self, field, option):
        accepted = option in {"tracking"}
        return accepted or super()._valid_field_parameter(field, option)


class ResPartner(models.Model):
    _inherit = "res.partner"

    def action_follow(self):
        return True
