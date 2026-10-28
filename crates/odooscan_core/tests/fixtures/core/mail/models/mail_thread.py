from odoo import fields, models


class MailThread(models.AbstractModel):
    _name = "mail.thread"

    message_ids = fields.One2many("res.partner", "parent_id")


class ResPartner(models.Model):
    _inherit = "res.partner"

    def action_follow(self):
        return True
