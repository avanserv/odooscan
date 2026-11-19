from odoo import api, fields, models
from odoo.addons.field_classes.orm.vector import Mark, Pair


# When Pair is the Many2many, Odoo names the table of country_ids after x_lens and res_country and
# creates it, so this model reads from a table that exists.
class LensCountry(models.Model):
    _name = "x.lens.country"
    _description = "Lens country link"
    _table = "res_country_x_lens_rel"
    _auto = False

    x_lens_id = fields.Many2one("x.lens", readonly=True)
    res_country_id = fields.Many2one("res.country", readonly=True)


# Pair takes its comodel by position as a Many2many does, so Odoo names this table after x_lens
# and res_country_group.
class LensRegion(models.Model):
    _name = "x.lens.region"
    _description = "Lens region link"
    _table = "res_country_group_x_lens_rel"
    _auto = False

    x_lens_id = fields.Many2one("x.lens", readonly=True)


class Lens(models.Model):
    _inherit = "x.lens"

    country_ids = Pair(comodel_name="res.country")
    region_ids = Pair("res.country.group")
    shade = Mark(compute="_compute_shade")

    # The scan does not judge the type of Mark, a Char or a Many2one, so it counts 5 as a value
    # shade takes: every lens has shade before int() raises.
    @api.depends("code")
    def _compute_shade(self):
        for lens in self:
            try:
                lens.shade = 5
                lens.shade = int(lens.code)
            except ValueError:
                pass
