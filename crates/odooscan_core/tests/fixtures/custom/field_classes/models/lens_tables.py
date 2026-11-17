from odoo import fields, models
from odoo.addons.field_classes.orm.vector import Mark, Pair


# Mark is a Char or a Many2one, and neither keeps rows in a table, so nothing creates this one.
class LensPartner(models.Model):
    _name = "x.lens.partner"
    _description = "Lens partner link"
    _table = "res_partner_x_lens_rel"
    _auto = False

    x_lens_id = fields.Many2one("x.lens", readonly=True)
    res_partner_id = fields.Many2one("res.partner", readonly=True)


# When Pair is the Many2many, Odoo names its table res_currency_x_lens_rel, not this one.
class LensCurrency(models.Model):
    _name = "x.lens.currency"
    _description = "Lens currency link"
    _table = "x_lens_currency_rel"
    _auto = False

    x_lens_id = fields.Many2one("x.lens", readonly=True)
    res_currency_id = fields.Many2one("res.currency", readonly=True)


class Lens(models.Model):
    _inherit = "x.lens"

    partner_id = Mark(comodel_name="res.partner")
    currency_ids = Pair(comodel_name="res.currency")
