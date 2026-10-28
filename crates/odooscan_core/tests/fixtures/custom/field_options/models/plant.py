from odoo import api, fields, models


class Base(models.AbstractModel):
    _inherit = "base"

    def _valid_field_parameter(self, field, option):
        return option in {"greenhouse_bed"} or super()._valid_field_parameter(field, option)


class GreenhousePlant(models.Model):
    _name = "greenhouse.plant"
    _description = "Greenhouse plant"

    name = fields.Char()
    stage = fields.Char(tracking=True)
    copy = fields.Boolean()
    env = fields.Char()
    parent_id = fields.Many2one("greenhouse.plant")
    child_ids = fields.One2many("greenhouse.plant", "parent_id")
    wizard_ids = fields.One2many("greenhouse.wizard", "plant_id")
    total = fields.Float(compute="_compute_total", store=True)
    height = fields.Float(compute="_compute_height", store=True, recursive=True)
    weight = fields.Float(compute="_compute_weight", store=True, depends=["child_ids"])

    @api.depends("child_ids", "child_ids.total")
    def _compute_total(self):
        for plant in self:
            plant.total = len(plant.child_ids)

    @api.depends("child_ids.height")
    def _compute_height(self):
        for plant in self:
            plant.height = max(plant.child_ids.mapped("height"), default=0.0)

    @api.depends("child_ids.weight")
    def _compute_weight(self):
        for plant in self:
            plant.weight = len(plant.child_ids)


class GreenhousePlantStage(models.Model):
    _inherit = "greenhouse.plant"

    stage = fields.Char(tracking=False)

    @api.depends("parent_id.total")
    def _compute_total(self):
        return super()._compute_total()


class GreenhouseLog(models.Model):
    _name = "greenhouse.log"
    _description = "Greenhouse log"
    _inherit = ["mail.thread"]

    state = fields.Char(tracking=True)


class GreenhouseMixin(models.AbstractModel):
    _name = "greenhouse.mixin"
    _description = "Greenhouse mixin"

    note = fields.Char(tracking=True)
    stamped_on = fields.Datetime(string="Last Updated on")
    ref = fields.Char(string="ID")
    parent_id = fields.Many2one("greenhouse.mixin")
    level = fields.Integer(compute="_compute_level")

    @api.depends("parent_id.level")
    def _compute_level(self):
        for record in self:
            record.level = record.parent_id.level + 1


class GreenhouseSensor(models.Model):
    _name = "greenhouse.sensor"
    _description = "Greenhouse sensor"

    reading = fields.Float(tracking=True)

    def _valid_field_parameter(self, field, option):
        known = super()._valid_field_parameter(field, option)
        return known or option in ("tracking", "sensor_unit")


class GreenhouseWizard(models.TransientModel):
    _name = "greenhouse.wizard"
    _description = "Greenhouse wizard"

    plant_id = fields.Many2one("greenhouse.plant")
    count = fields.Integer(compute="_compute_count")

    @api.depends("plant_id.wizard_ids.count")
    def _compute_count(self):
        for wizard in self:
            wizard.count = len(wizard.plant_id.wizard_ids)
