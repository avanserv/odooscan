from odoo import models

from odoo.addons.prov_python.tools import gadget_total


class ResPartner(models.Model):
    _inherit = "res.partner"

    def _gadget_summary(self):
        bare = "prov_bare" in self.env.context
        tiers = self.filtered(lambda p: p.type == "gadget_tier")
        gadgets = self.env["prov.gadget"].search([])
        widgets = self.env["under.widget"].search([])
        group = self.env.ref("prov_ids.gadget_group")
        weights = [p.gadget_weight for p in self] + [p._gadget_rank() for p in tiers]
        zones = self.env.company.short_name
        handled = self.env["ir.http"]._handle_error(ValueError("gadget"))
        return bare, gadgets, widgets, group, gadget_total(weights), self.mapped("name"), zones, handled
