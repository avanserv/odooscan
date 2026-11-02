from odoo import fields, models


class ThemeView(models.AbstractModel):
    _name = "theme.ir.ui.view"
    _description = "Theme view"

    name = fields.Char()
