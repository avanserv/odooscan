from odoo import fields, models

LABELS = {}


class UnreadFormsAnnotated(models.Model):
    _name: str = "unread.forms.annotated"
    _description = "Annotated form"

    note = fields.Char()

    def action_log(self):
        print "logged"


class UnreadFormsChained(models.Model):
    LABELS["chained"] = _name = "unread.forms.chained"
    _description = "Chained form"

    note = fields.Char()


class UnreadFormsLine(models.Model): _name = "unread.forms.line"; _description = "One-line form"


class UnreadFormsMeta(models.Model):
    class Meta: pass
    _name = "unread.forms.meta"
    _description = "Form after a one-line class"

    note = fields.Char()
