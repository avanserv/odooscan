from odoo import api, fields, models

LOOSE_COMODEL = "modifier.line"


class PlainHooks:
    """Leaves modifier.hook open to fields the scan cannot read."""


class ModifierTicket(models.Model):
    _name = "modifier.ticket"
    _description = "Modifier ticket"

    name = fields.Char()
    state = fields.Selection([("new", "New"), ("done", "Done")])
    line_ids = fields.One2many("modifier.line", "ticket_id")
    tag_ids = fields.Many2many("modifier.ticket.tag")
    hook_ids = fields.Many2many("modifier.hook")
    bound_ids = fields.Many2many("modifier.bound")
    loose_ids = fields.Many2many(LOOSE_COMODEL)

    def action_close(self):
        return True


class ModifierLine(models.Model):
    _name = "modifier.line"
    _description = "Modifier line"

    name = fields.Char()
    quantity = fields.Integer()
    ticket_id = fields.Many2one("modifier.ticket")
    tag_ids = fields.Many2many("modifier.line.tag")

    def action_split(self):
        return True


class ModifierTicketTag(models.Model):
    _name = "modifier.ticket.tag"
    _description = "Modifier ticket tag"

    name = fields.Char()
    owner_id = fields.Many2one("modifier.ticket")

    @api.private
    def action_fade(self):
        return True


class ModifierLineTag(models.Model):
    _name = "modifier.line.tag"
    _description = "Modifier line tag"

    name = fields.Char()
    color = fields.Integer()
    owner_id = fields.Many2one("modifier.line")

    def action_fade(self):
        return True

    def action_tint(self):
        return True


class ModifierHook(PlainHooks, models.Model):
    _name = "modifier.hook"
    _description = "Modifier hook"

    name = fields.Char()


class ModifierBound(models.Model):
    _name = "modifier.bound"
    _inherit = ["modifier.ghost"]
    _description = "Modifier bound"

    name = fields.Char()
