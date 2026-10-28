from odoo import api, fields, models


class GardenPlot(models.Model):
    _name = "garden.plot"
    _description = "Garden plot"

    name = fields.Char()
    partner_id = fields.Many2one("res.partner")
    note = fields.Char()

    @api.onchange("partner_id.country_id")
    def _onchange_partner_country(self):
        self.note = self.partner_id.country_id.name

    @api.onchange("partner_id")
    def _onchange_partner(self):
        self.note = self.partner_id.name

    @api.constrains("partner_id.nickname")
    def _check_partner_nickname(self):
        for plot in self:
            plot.note = plot.partner_id.name
