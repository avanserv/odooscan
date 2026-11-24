from activity_kit import ACTIVITY_MIXINS, ActivityBase

from odoo import fields, models


class WindowBridgedPlain(models.Model, ActivityBase):
    _inherit = "window.bridged.plain"


class WindowBridgedListed(models.Model):
    _name = "window.bridged.listed"
    _inherit = ["window.bridged.listed"] + ACTIVITY_MIXINS


class WindowBridgedMixin(models.Model):
    _name = "window.bridged.mixin"
    _inherit = ["window.bridged.mixin", "window.tracker.mixin"]


class WindowBridgedDelegate(models.Model):
    _inherit = "window.bridged.delegate"
    _inherits = {"window.tracker": "tracker_id"}

    tracker_id = fields.Many2one("window.tracker", required=True, ondelete="cascade")


class WindowBridgedRead(models.Model):
    _inherit = "window.bridged.read"

    note = fields.Char()
