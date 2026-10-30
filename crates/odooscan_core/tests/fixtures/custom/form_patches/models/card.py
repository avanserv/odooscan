from odoo import fields, models


class FormCardPrintable(models.AbstractModel):
    _name = "form.card.printable"
    _description = "Printable form card"

    def action_print_card(self):
        return True


class FormCard(models.Model):
    _name = "form.card"
    _description = "Form card"
    _inherit = ["form.card.printable"]


class FormCardSleeve(models.Model):
    _name = "form.card.sleeve"
    _description = "Form card sleeve"
    _inherits = {"form.card": "card_id"}

    card_id = fields.Many2one("form.card", required=True, ondelete="cascade")
