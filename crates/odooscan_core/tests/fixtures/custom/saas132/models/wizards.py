from odoo import fields, models


class SaasLaterPassWizard(models.TransientModel):
    _name = "saas.later.pass.wizard"
    _description = "SaaS later pass wizard"

    name = fields.Char()


class SaasLaterNoteWizard(models.TransientModel):
    _name = "saas.later.note.wizard"
    _description = "SaaS later note wizard"

    name = fields.Char()
