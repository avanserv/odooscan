from odoo import api, fields, models, tools


class ResPartner(models.Model):
    _inherit = "res.partner"
    _sql_constraints = [("name_uniq", "unique(name)", "Name must be unique")]

    def action_reset(self):
        self._cr.execute("UPDATE res_partner SET name = NULL WHERE id IN %s", (tuple(self.ids),))


class ModernNote(models.AbstractModel):
    _name = "modern.note"
    _description = "Tracked note"

    note = fields.Char(tracking=True)


class ResPartnerCreate(models.Model):
    _inherit = "res.partner"

    @api.model
    def create(self, vals):
        vals.setdefault("ref", vals.get("name"))
        return super().create(vals)


class ResPartnerBatchCreate(models.Model):
    _inherit = "res.partner"

    @api.model
    def create(self, vals_list):
        for vals in vals_list:
            vals.setdefault("ref", vals.get("name"))
        return super().create(vals_list)


class ResPartnerKiosk(models.Model):
    _inherit = "res.partner"

    type = fields.Selection((("contact", "Contact"), ("invoice", "Invoice")), selection_add=[("kiosk",)])


class ModernBadge(models.AbstractModel):
    _name = "modern.badge"
    _description = "Badge holder"
    _inherits = {"res.partner": "partner_id"}

    partner_id = fields.Many2one("res.partner", required=True, ondelete="set null")
    country_id = fields.Many2one("res.country", delegate=True)


class ResPartnerStamp(models.Model):
    _inherit = "res.partner"

    touched_on = fields.Datetime(string="Last Modified on")


class ModernSeal(models.AbstractModel):
    _name = "modern.seal"
    _description = "Seal"

    ref = fields.Char(string="ID")


class ResPartnerCachedRates(models.Model):
    _inherit = "res.partner"

    @tools.ormcache("int(time.monotonic() // 900)")
    def _quarter_hour_rates(self):
        return {}

    @tools.ormcache("method.__name__")
    def _named_by_method(self):
        return self._name


class PartnerTier(models.Model):
    _inherit = "res.partner"

    tier = fields.Selection(selection=(("gold", "Gold"), ("silver", "Silver")))
