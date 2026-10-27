from odoo import fields, models


class MailThread(models.AbstractModel):
    _name = "mail.thread"

    message_ids = fields.One2many("res.partner", "parent_id")
