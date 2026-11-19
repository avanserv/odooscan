import peewee

from odoo import fields
from odoo.models import Model


class Ledger(peewee.Model):
    entry = peewee.CharField()


class LedgerLine(Ledger):
    amount = peewee.IntegerField()


class DeskClamp(Model):
    _description = "Desk clamp"

    grip = fields.Integer()


class ArmMixin:
    _inherit = []


class DeskArm(ArmMixin, Model):
    _description = "Desk arm"

    reach = fields.Integer()
