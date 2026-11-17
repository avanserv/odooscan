from odoo import fields, models
from odoo.osv import osv


class UnreadKindsWizard(models.TransientModel):
    _name = "unread.kinds.wizard"

    note = fields.Char()

    def action_log(self):
        print "logged"


class UnreadKindsNote(osv.osv_memory):
    _name = "unread.kinds.note"


class UnreadKindsDesk(models.Model):
    _name = "unread.kinds.desk"


class UnreadKindsLamp(models.Model):
    wattage = fields.Integer()
