from odoo import fields, models

DATE_FIELD = "planned_on"


class WindowExtended(models.Model):
    _inherit = "window.extended"

    date_start = fields.Date()
    date_stop = fields.Date()


class WindowPostponed(models.Model):
    _inherit = "window.postponed"
    _date_name = "planned_on"

    planned_on = fields.Date()
    date_delay = fields.Float()


class WindowGuessed(models.Model):
    _inherit = "window.guessed"
    _date_name = DATE_FIELD


class WindowUnplanned(models.Model):
    _inherit = "window.unplanned"
    _date_name = "planned_on"
