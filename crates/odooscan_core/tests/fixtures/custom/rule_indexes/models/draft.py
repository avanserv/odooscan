from odoo import fields, models


class DeskCaseDraft(models.Model):
    _inherit = "desk.case.draft"

    stage = fields.Char(
