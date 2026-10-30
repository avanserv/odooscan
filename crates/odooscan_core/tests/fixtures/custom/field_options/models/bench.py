from odoo import _lt, fields, models


class GreenhouseBench(models.Model):
    _name = "greenhouse.bench"
    _description = "Greenhouse bench"

    partner_id = fields.Many2one("res.partner")
    partner_ids = fields.Many2many("res.partner")
    planted_on = fields.Datetime("Created on")
    partner_name = fields.Char(related="partner_id.name")
    partner_label = fields.Char(string="Partner Name")
    summary = fields.Char(string=_lt("Notes"))
    notes = fields.Char()
    write_date = fields.Datetime(string="Stamped")
    changed_on = fields.Datetime(string="Last Updated on")


class GreenhouseBenchIndex(models.Model):
    _inherit = "greenhouse.bench"

    create_date = fields.Datetime(index="btree")


class GreenhouseReading(models.Model):
    _name = "greenhouse.reading"
    _description = "Greenhouse reading"
    _log_access = False

    taken_on = fields.Datetime(string="Created on")


class GreenhouseReport(models.Model):
    _name = "greenhouse.report"
    _description = "Greenhouse report"
    _auto = False  # odooscan: ignore[OD226]

    author_id = fields.Many2one("res.partner", string="Created by")


class ResPartner(models.Model):
    _inherit = "res.partner"

    common_name = fields.Char(string="Name")


class GreenhouseStamp(models.AbstractModel):
    _name = "greenhouse.stamp"
    _description = "Greenhouse stamp"

    write_date = fields.Datetime(string="Stamped")


class GreenhouseTray(models.Model):
    _name = "greenhouse.tray"
    _description = "Greenhouse tray"
    _inherit = ["greenhouse.stamp"]

    create_date = fields.Datetime()
    filled_on = fields.Datetime(string="Create Date")
    checked_on = fields.Datetime(string="Stamped")
    _condition_to_sql = fields.Char()
