from odoo import fields, models

DATE_FIELD = "planned_on"


class WindowPlain(models.Model):
    _name = "window.plain"
    _description = "Window plain"

    name = fields.Char()


class WindowDated(models.Model):
    _name = "window.dated"
    _description = "Window dated"

    date = fields.Date()


class WindowSpan(models.Model):
    _name = "window.span"
    _description = "Window span"

    date_start = fields.Date()
    date_stop = fields.Date()


class WindowTimed(models.Model):
    _name = "window.timed"
    _description = "Window timed"
    _date_name = "planned_on"

    planned_on = fields.Date()
    date_delay = fields.Float()


class WindowNamed(models.Model):
    _name = "window.named"
    _description = "Window named"
    _date_name = DATE_FIELD

    name = fields.Char()


class WindowDrawn(models.Model):
    _name = "window.drawn"
    _description = "Window drawn"

    name = fields.Char()

    def _get_default_gantt_view(self):
        return None


class WindowShown(models.Model):
    _name = "window.shown"
    _description = "Window shown"

    name = fields.Char()


class WindowDerived(models.Model):
    _name = "window.derived"
    _description = "Window derived"

    name = fields.Char()


class WindowBridged(models.Model):
    _name = "window.bridged"
    _description = "Window bridged"

    name = fields.Char()


class WindowExtended(models.Model):
    _name = "window.extended"
    _description = "Window extended"

    name = fields.Char()
