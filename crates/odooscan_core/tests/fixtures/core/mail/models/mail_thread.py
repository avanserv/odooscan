from odoo import fields, models


class MailThread(models.AbstractModel):
    _name = "mail.thread"

    message_ids = fields.One2many("res.partner", "parent_id")

    def _valid_field_parameter(self, field, option):
        accepted = option in {"tracking"}
        return accepted or super()._valid_field_parameter(field, option)


class MailActivityMixin(models.AbstractModel):
    _name = "mail.activity.mixin"

    activity_ids = fields.One2many("res.partner", "parent_id")


class Base(models.AbstractModel):
    _inherit = "base"

    def _get_default_activity_view(self):
        return None


class ResPartner(models.Model):
    _inherit = "res.partner"

    def action_follow(self):
        return True
