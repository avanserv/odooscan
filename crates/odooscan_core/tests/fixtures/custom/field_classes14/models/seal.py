from odoo import fields, models


class SealOld(models.Model):
    _name = "x.seal.old"
    _description = "Old seal"

    photo = fields.Image(required=True)
    kind = fields.Reference([("res.partner", "Partner")], required=True)
    ref = fields.Many2oneReference(model_field="res_model", required=True)
    back = fields.Selection([("plain", "Plain")], required=True)
    res_model = fields.Char()


# On 14.0 the registry walks back from the last definition of a field while each class derives from
# the last one's, as Image from Binary, Reference from Selection and Many2oneReference from Integer:
# photo, kind and ref keep required=True, and back loses it, as Selection does not derive from
# Reference.
class SealOldExtension(models.Model):
    _inherit = "x.seal.old"

    # Image keeps the type of Binary, so photo gets no OD205.
    photo = fields.Binary(string="P")
    kind = fields.Selection([("plain", "Plain")], string="K")
    ref = fields.Integer(string="R")
    back = fields.Reference([("res.partner", "Partner")], string="B")
