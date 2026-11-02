from odoo import fields, models


class PlainHooks:
    def action_hook(self):
        return True


class ButtonPlain(PlainHooks, models.AbstractModel):
    _name = "button.plain"
    _description = "Button plain"

    name = fields.Char()


class ButtonGhosted(models.AbstractModel):
    _name = "button.ghosted"
    _description = "Button ghosted"
    _inherit = ["ghost.mixin"]

    name = fields.Char()
