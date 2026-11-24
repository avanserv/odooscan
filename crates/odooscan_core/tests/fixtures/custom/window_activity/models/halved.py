from odoo import fields, models

PARENTS = {"res.partner": "partner_id"}


class WindowHalved(models.Model):
    _name = "window.halved"
    _description = "Window halved"

    name = fields.Char()
    activity_ids = fields.Many2many("res.partner")


class WindowStated(models.Model):
    _name = "window.stated"
    _description = "Window stated"

    name = fields.Char()
    activity_ids = fields.Many2many("res.partner")
    activity_state = fields.Selection([("overdue", "Overdue"), ("today", "Today"), ("planned", "Planned")])


class WindowHalvedUnread(models.Model):
    _name = "window.halved.unread"
    _description = "Window halved unread"
    _inherits = PARENTS

    partner_id = fields.Many2one("res.partner", required=True, ondelete="cascade")
    activity_ids = fields.Many2many("res.partner")
