from functools import partial

from odoo import api, fields, models


class FormCardPrintable(models.AbstractModel):
    _name = "form.card.printable"
    _description = "Printable form card"

    def action_print_card(self):
        return True

    @api.private
    def action_stamp_card(self):
        return True

    @classmethod
    def card_sizes(cls):
        return ["a6", "a7"]

    @staticmethod
    def card_margin(size):
        return 4 if size == "a7" else 6

    card_title = partial(str.title)


class FormCard(models.Model):
    _name = "form.card"
    _description = "Form card"
    _inherit = ["form.card.printable"]


class FormCardSleeve(models.Model):
    _name = "form.card.sleeve"
    _description = "Form card sleeve"
    _inherits = {"form.card": "card_id"}

    card_id = fields.Many2one("form.card", required=True, ondelete="cascade")
