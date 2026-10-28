from odoo import api, fields, models


class ResPartner(models.Model):
    _inherit = "res.partner"

    theme_id = fields.Many2one("web.theme")
    parent_color = fields.Char(compute="_compute_parent_color")

    @api.depends("parent_id.web_color")
    def _compute_parent_color(self):
        for partner in self:
            partner.parent_color = partner.parent_id.web_color
