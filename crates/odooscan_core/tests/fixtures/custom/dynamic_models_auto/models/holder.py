from odoo import fields, models


# dynamic_models_auto_late builds auto.late.large but loads after this module: the paths Odoo reads
# at search time find its fields, those it resolves as this module loads do not.
class AutoHolder(models.Model):
    _name = "auto.holder"
    _rec_names_search = ["late_id.size"]

    late_id = fields.Many2one("auto.late.large", domain="[('size', '>', 0)]")
    late_size = fields.Integer(related="late_id.size")
