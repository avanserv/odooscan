from odoo import fields, models


class ButtonDesk(models.Model):
    _name = "button.desk"
    _description = "Button desk"

    name = fields.Char()

    def init(self):
        super().init()

    def with_context(self, *args, **kwargs):
        return super().with_context(*args, **kwargs)

    def action_open(self):
        return True
