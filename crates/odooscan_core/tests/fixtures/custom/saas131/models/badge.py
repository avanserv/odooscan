from odoo import fields, models


class SaasBand(models.AbstractModel):
    _name = "saas.band"
    _description = "SaaS band"

    model_id = fields.Many2one("ir.model", delegate=True, required=True)


class SaasPassWizard(models.TransientModel):
    _name = "saas.pass.wizard"
    _description = "SaaS pass wizard"

    name = fields.Char()


class SaasNoteWizard(models.TransientModel):
    _name = "saas.note.wizard"
    _description = "SaaS note wizard"

    name = fields.Char()


class SaasRegister(models.Model):
    _name = "saas.register"
    _description = "SaaS register"

    name = fields.Char()
