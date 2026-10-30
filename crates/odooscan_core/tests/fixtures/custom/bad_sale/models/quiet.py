from odoo import fields, models


class QuietMixin(models.AbstractModel):
    _name = "bad.quiet.mixin"

    label = fields.Char()


class QuietPublishedMixin(QuietMixin):
    _name = "bad.quiet.published.mixin"


class QuietWizard(models.TransientModel):
    _name = "bad.quiet.wizard"
    _inherit = ["bad.quiet.mixin"]

    label = fields.Text()
    partner_id = fields.Many2one("res.partner")
    partner_name = fields.Char(related="partner_id.name", readonly=False, inverse="_inverse_partner_name")

    def action_optional(self):
        if "bad.optional" in self.env:
            self.env["bad.optional"].search([])
        if self.env.get("bad.other") is not None:
            self.env["bad.other"].search([])
        return "other_id" in self._fields and self.env["bad.third"]

    def action_x_partner_names(self):
        self.env.cr.execute("SELECT p.x_bogus FROM res_partner p")
        return self.env.cr.fetchall()
