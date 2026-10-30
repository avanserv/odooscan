from odoo import fields, models


class ResPartner(models.Model):
    _inherit = "res.partner"

    home_country_id = fields.Many2one("res.country", delegate=True)
    x_origin_model = fields.Char()
    x_origin_id = fields.Many2oneReference(model_field="x_origin_model")
    x_parent_origin_id = fields.Many2oneReference(related="parent_id.x_origin_id", model_field="x_parent_model")


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


class ResPartnerLevel(models.Model):
    _inherit = "res.partner"

    x_level = fields.Integer(init_storage="_x_fill_level")

    def _x_fill_level(self):
        self.env.cr.execute("UPDATE res_partner SET x_level = 1 WHERE x_level IS NULL")


class ResPartnerQuery(models.Model):
    _inherit = "res.partner"

    _condition_to_sql = fields.Char()


class ModernStamp(models.AbstractModel):
    _name = "modern.stamp"
    _description = "Stamp"

    write_date = fields.Datetime(index=True)
    display_name = fields.Char(string="Stamp name")
    create_date = fields.Datetime(string="Stamped")


class ModernDesk(models.Model):
    _name = "modern.desk"
    _description = "Desk"
    _inherit = ["modern.stamp"]

    touched_on = fields.Datetime(string="Last Updated on")
    opened_on = fields.Datetime(string="Created on")
    shown_as = fields.Char(string="Stamp name")


class ModernTag(models.AbstractModel):
    _name = "modern.tag"
    _description = "Tag"

    caption = fields.Char(string="Display Name")


class ModernShelf(models.Model):
    _name = "modern.shelf"
    _description = "Shelf"
    _inherit = ["modern.tag"]

    display_name = fields.Char(compute="_compute_display_name")

    def _compute_display_name(self):
        for shelf in self:
            shelf.display_name = shelf.caption
