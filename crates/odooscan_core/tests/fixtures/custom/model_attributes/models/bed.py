from odoo import fields, models


class GardenBed(models.Model):
    _name = "garden.bed"
    _description = "Garden bed"
    _parent_name = "parent_bed_id"
    _fold_name = "folded"
    _active_name = "x_active"
    _rec_names_search = ["name", "partner_id.name", "code"]

    name = fields.Char()
    parent_id = fields.Many2one("garden.bed")
    partner_id = fields.Many2one("res.partner")
    stage_id = fields.Many2one("garden.stage", group_expand="_expand_stages")
    season_id = fields.Many2one("garden.stage", group_expand="_read_group_expand_full")
    soil = fields.Selection(selection="_selection_soils")
    size = fields.Selection("_selection_sizes", string="Size")
    exposure = fields.Selection("_selection_exposures")
    origin = fields.Reference("_selection_origins")
    shade = fields.Selection([("full", "Full"), ("partial", "Partial")], group_expand=True)
    res_model = fields.Char()
    res_id = fields.Many2oneReference(model_field="res_model")
    owner_id = fields.Many2oneReference(model_field="owner_model")
    parent_res_id = fields.Many2oneReference(related="parent_id.res_id", model_field="parent_model")

    def _selection_exposures(self):
        return [("sun", "Sun"), ("shade", "Shade")]


class GardenStage(models.Model):
    _name = "garden.stage"
    _description = "Garden stage"
    _fold_name = "fold"

    name = fields.Char()
    fold = fields.Boolean()


class GardenTagged(models.AbstractModel):
    _name = "garden.tagged"
    _description = "Tagged by a garden"
    _parent_name = "garden_parent_id"

    tag_id = fields.Many2oneReference(model_field="tag_model")


class SeasonChoices:
    def _selection_seasons(self):
        return [("spring", "Spring"), ("autumn", "Autumn")]

    def _compute_beds(self):
        for tray in self:
            tray.beds = 0


class GardenTray(models.Model, SeasonChoices):
    _name = "garden.tray"
    _description = "Garden tray"

    season = fields.Selection("_selection_seasons")
    beds = fields.Integer(compute="_compute_beds")
