from odoo import fields, models

CRATE_OPTIONS = {"relation": "x_crate_tag_rel", "string": "Tags"}


class Crate(models.Model):
    _name = "x.crate"
    _description = "Crate"

    # The keywords may name any table, so it may be the one x.crate.tag reads.
    tag_ids = fields.Many2many("res.country", **CRATE_OPTIONS)


class CrateTag(models.Model):
    _name = "x.crate.tag"
    _description = "Crate tag link"
    _table = "x_crate_tag_rel"
    _auto = False

    x_crate_id = fields.Many2one("x.crate", readonly=True)
    res_country_id = fields.Many2one("res.country", readonly=True)
