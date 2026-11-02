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


class WindowCloned(models.Model):
    _name = "window.cloned"
    _description = "Window cloned"

    name = fields.Char()


class WindowAdopted(models.Model):
    _name = "window.adopted"
    _description = "Window adopted"

    name = fields.Char()


class WindowRelayed(models.Model):
    _name = "window.relayed"
    _description = "Window relayed"

    name = fields.Char()


class WindowSought(models.Model):
    _name = "window.sought"
    _description = "Window sought"

    name = fields.Char()


class WindowSifted(models.Model):
    _name = "window.sifted"
    _description = "Window sifted"

    name = fields.Char()


class WindowBridged(models.Model):
    _name = "window.bridged"
    _description = "Window bridged"

    name = fields.Char()


class WindowExtended(models.Model):
    _name = "window.extended"
    _description = "Window extended"

    name = fields.Char()


class WindowOutside(models.Model):
    _name = "window.outside"
    _description = "Window outside"

    name = fields.Char()


class WindowRooted(models.Model):
    _name = "window.rooted"
    _description = "Window rooted"

    name = fields.Char()


class WindowStepped(models.Model):
    _name = "window.stepped"
    _description = "Window stepped"

    name = fields.Char()


class WindowGhosted(models.Model):
    _name = "window.ghosted"
    _description = "Window ghosted"

    name = fields.Char()


class WindowNameless(models.Model):
    _name = "window.nameless"
    _description = "Window nameless"

    name = fields.Char()


class WindowScheduled(models.Model):
    _name = "window.scheduled"
    _description = "Window scheduled"

    name = fields.Char()


class WindowTracked(models.Model):
    _name = "window.tracked"
    _description = "Window tracked"

    name = fields.Char()
    activity_ids = fields.Many2many("res.partner")
