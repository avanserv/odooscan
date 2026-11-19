from odoo import fields, models


class Seal(models.Model):
    _name = "x.seal"
    _description = "Seal"

    photo = fields.Binary(required=True)
    kind = fields.Selection([("plain", "Plain")], required=True)
    ref = fields.Integer(required=True)
    back = fields.Reference([("res.partner", "Partner")], required=True)
    res_model = fields.Char()


# On 20.0 the registry keeps what a definition sets when the class of the last one derives from its
# class, as Image from Binary, Reference from Selection and Many2oneReference from Integer: photo,
# kind and ref keep required=True, and back loses it, as Selection does not derive from Reference.
class SealExtension(models.Model):
    _inherit = "x.seal"

    # OD205 on both photo lines is the known Binary and Image false positive: Image keeps the binary
    # type of Binary, but OD205 compares class names. The OD205 follow-up removes both lines.
    photo = fields.Image(string="P")
    kind = fields.Reference([("res.partner", "Partner")], string="K")
    ref = fields.Many2oneReference(model_field="res_model", string="R")
    back = fields.Selection([("plain", "Plain")], string="B")
