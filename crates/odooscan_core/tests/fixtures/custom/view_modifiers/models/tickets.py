from odoo import fields, models


class ModifierTicket(models.Model):
    _name = "modifier.ticket"
    _description = "Modifier ticket"

    name = fields.Char()
    state = fields.Selection([("new", "New"), ("done", "Done")])
    line_ids = fields.One2many("modifier.line", "ticket_id")


class ModifierLine(models.Model):
    _name = "modifier.line"
    _description = "Modifier line"

    name = fields.Char()
    quantity = fields.Integer()
    ticket_id = fields.Many2one("modifier.ticket")
