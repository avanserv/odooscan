from odoo import fields, models


class PlainContacts:
    def action_contact(self):
        return True


class ContactPartner(PlainContacts, models.Model):
    _inherit = "res.partner"


class ContactUser(models.Model):
    _inherit = "res.users"
    _inherits = {"res.partner": "partner_id"}

    partner_id = fields.Many2one("res.partner", required=True, ondelete="cascade")
    nickname = fields.Char(compute="_compute_nicknmae")

    def _compute_nickname(self):
        for user in self:
            user.nickname = user.login


class ButtonCopied(models.AbstractModel):
    _name = "button.copied"
    _description = "Button copied"
    _inherit = ["res.partner"]
