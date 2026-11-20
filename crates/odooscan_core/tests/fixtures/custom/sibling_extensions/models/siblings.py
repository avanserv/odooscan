from odoo import fields, models


class GhostLampColour(models.Model):
    _inherit = "ghost.lamp"


class GhostLampSize(models.Model):
    _inherit = "ghost.lamp"


class TaskColour(models.Model):
    _inherit = "project.task"


class TaskSize(models.Model):
    _inherit = "project.task"


class PartnerColour(models.Model):
    _inherit = "res.partner"

    lamp_colour = fields.Char()


class PartnerSize(models.Model):
    _inherit = "res.partner"

    lamp_size = fields.Integer()


class SiblingLamp(models.AbstractModel):
    _name = "sibling.lamp"
    _description = "Sibling lamp"


class SiblingLampColour(models.AbstractModel):
    _inherit = "sibling.lamp"
