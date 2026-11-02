from odoo import fields, models


class ServerJob(models.Model):
    _name = "server.job"
    _description = "Server job"

    name = fields.Char()

    def _run_daily(self):
        return True

    def action_close(self):
        return True


class ServerNote(models.Model):
    _name = "server.note"
    _description = "Server note"

    name = fields.Char()

    def action_pin(self):
        return True
