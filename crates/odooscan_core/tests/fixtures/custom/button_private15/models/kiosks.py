from odoo import fields, models


class ButtonKiosk(models.Model):
    _name = "button.kiosk"
    _description = "Button kiosk"

    name = fields.Char()

    def action_open(self):
        return True
