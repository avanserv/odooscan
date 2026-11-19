from odoo import fields, models


class ButtonTally(models.AbstractModel):
    _name = "button.tally"
    _description = "Button tally"

    name = fields.Char()

    @classmethod
    def action_tally(cls):
        return 3
