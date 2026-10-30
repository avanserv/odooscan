from odoo import fields, models


class ResPartner(models.Model):
    _inherit = "res.partner"

    home_country_id = fields.Many2one("res.country", delegate=True)


class ModernNote(models.AbstractModel):
    _name = "modern.note"
    _description = "Tracked note"

    note = fields.Char(tracking=True)


class ModernTrail(models.AbstractModel):
    _name = "modern.trail"
    _description = "Change trail"

    def _valid_field_parameter(self, field, option):
        wanted = ("tracking", "trail_level")
        return option in wanted or super()._valid_field_parameter(field, option)


class ModernChatter(models.AbstractModel):
    _name = "modern.chatter"
    _description = "Chatter"
    _inherit = ["modern.trail"]


class ModernTicket(models.AbstractModel):
    _name = "modern.ticket"
    _description = "Ticket"
    _inherit = ["modern.chatter"]

    stage = fields.Char(tracking=True)
    ref = fields.Char(string="ID")
