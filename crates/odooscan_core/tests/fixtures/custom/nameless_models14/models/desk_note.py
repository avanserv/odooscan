from odoo import fields, models


class NoteBase:
    _inherit = ["res.partner"]


class DeskNote(NoteBase, models.Model):
    body = fields.Text()


class DeskTray(models.Model):
    depth = fields.Integer()
