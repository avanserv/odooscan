from odoo import api, fields, models
from odoo.exceptions import UserError

BANNER_OPTIONS = {"translate": True}


class ResPartnerTaglineHook(models.Model):
    _inherit = "res.partner"

    def _compute_tagline(self):
        for partner in self:
            partner.tagline = partner.name


class ResPartner(models.Model):
    _inherit = "res.partner"

    handle = fields.Char()
    tagline = fields.Char(compute="_compute_tagline")
    initials = fields.Char(compute="_compute_initials")
    badge = fields.Char(compute="_compute_badge")
    stored_initials = fields.Char(compute="_compute_stored_initials", store=True)
    editable_initials = fields.Char(compute="_compute_editable_initials", readonly=False)
    inverse_initials = fields.Char(compute="_compute_inverse_initials", inverse="_inverse_initials")
    greeting = fields.Char(compute="_compute_greeting")
    motto = fields.Char(compute="_compute_motto")
    salutation = fields.Char(compute="_compute_salutation")
    sign = fields.Char(compute="_compute_sign")
    level = fields.Char(compute="_compute_level")
    age_years = fields.Integer(compute="_compute_age_years")
    first_child = fields.Char(compute="_compute_first_child")
    checked_name = fields.Char(compute="_compute_checked_name")
    company_label = fields.Char(compute="_compute_company_label")
    short_name = fields.Char(compute="_compute_short_name_from_handle")
    pen_name = fields.Char(compute="_compute_pen_name", store=False)
    parent_label = fields.Char(related="parent_id.name", compute="_compute_parent_label")
    banner = fields.Char(compute="_compute_banner", **BANNER_OPTIONS)

    @api.depends("name")
    def _compute_initials(self):
        for partner in self:
            if partner.name:
                partner.initials = "".join(word[0] for word in partner.name.split())

    @api.depends("name", "parent_id")
    def _compute_badge(self):
        for partner in self:
            if not partner.parent_id:
                continue
            partner.badge = f"{partner.parent_id.name}/{partner.name}"

    @api.depends("name")
    def _compute_stored_initials(self):
        for partner in self:
            if partner.name:
                partner.stored_initials = partner.name[:2]

    @api.depends("name")
    def _compute_editable_initials(self):
        for partner in self:
            if partner.name:
                partner.editable_initials = partner.name[:2]

    @api.depends("name")
    def _compute_inverse_initials(self):
        for partner in self:
            if partner.name:
                partner.inverse_initials = partner.name[:2]

    def _inverse_initials(self):
        pass

    @api.depends("name")
    def _compute_greeting(self):
        for partner in self:
            if not partner.greeting:
                partner.greeting = f"Hello {partner.name}"

    @api.depends("name")
    def _compute_motto(self):
        self.motto = False
        for partner in self:
            if partner.name:
                partner.motto = partner.name.upper()

    @api.depends("name")
    def _compute_salutation(self):
        for partner in self:
            if partner.name:
                partner.salutation = partner.name
            else:
                partner._fill_salutation()

    def _fill_salutation(self):
        self.salutation = "Dear customer"

    @api.depends("name")
    def _compute_sign(self):
        target = "sign"
        for partner in self:
            if partner.name:
                setattr(partner, target, partner.name[-1])

    @api.depends("type")
    def _compute_level(self):
        for partner in self:
            if partner.type == "contact":
                partner.level = "person"
            elif partner.type == "invoice":
                partner.level = "billing"

    @api.depends("handle")
    def _compute_age_years(self):
        for partner in self:
            try:
                years = int(partner.handle or "")
            except ValueError:
                partner.age_years = 0
            else:
                partner.age_years = years

    @api.depends("child_ids.name")
    def _compute_first_child(self):
        for partner in self:
            for child in partner.child_ids:
                if child.name:
                    partner.first_child = child.name
                    break
            else:
                partner.first_child = False

    @api.depends("name")
    def _compute_checked_name(self):
        for partner in self:
            if partner.name:
                partner.checked_name = partner.name.strip()
            else:
                raise UserError("A contact needs a name.")

    @api.depends("parent_id")
    def _compute_company_label(self):
        for partner in self.filtered("parent_id"):
            if partner.parent_id.name:
                partner.company_label = partner.parent_id.name

    @api.depends("name", "parent_id")
    def _compute_tagline(self):
        super()._compute_tagline()
        for partner in self:
            if partner.parent_id:
                partner.tagline = f"{partner.name} Ltd"

    @api.depends("handle")
    def _compute_short_name_from_handle(self):
        for partner in self:
            if partner.handle:
                partner.short_name = partner.handle

    @api.depends("handle")
    def _compute_pen_name(self):
        for partner in self:
            if partner.handle:
                partner.pen_name = partner.handle.title()

    @api.depends("parent_id")
    def _compute_parent_label(self):
        for partner in self:
            if partner.parent_id:
                partner.parent_label = partner.parent_id.name

    @api.depends("name")
    def _compute_banner(self):
        for partner in self:
            if partner.name:
                partner.banner = partner.name.upper()


class ResPartnerAudience(models.Model):
    _inherit = "res.partner"

    audience = fields.Char(compute="_compute_audience")
    name_hint = fields.Char(compute="_compute_name_hint")
    family_tag = fields.Char(compute="_compute_family_tag")
    first_letter = fields.Char(compute="_compute_first_letter")
    country_hint = fields.Char(compute="_compute_country_hint")
    welcome_line = fields.Char(compute="_compute_welcome_line")
    channel = fields.Char(compute="_compute_channel")
    last_name = fields.Char(compute="_compute_last_name")
    youngest_child = fields.Char(compute="_compute_youngest_child")

    @api.depends("type")
    def _compute_audience(self):
        for partner in self:
            if partner.type == "contact":
                partner.audience = "people"
            if partner.type == "invoice":
                partner.audience = "accounting"

    @api.depends("name")
    def _compute_name_hint(self):
        for partner in self:
            if partner.name:
                partner.name_hint = partner.name.split()[0]
            if partner.name:
                partner.name_hint = partner.name.lower()

    @api.depends("child_ids.name", "child_ids.country_id")
    def _compute_family_tag(self):
        for partner in self:
            for relative in partner.child_ids.filtered("name"):
                partner.family_tag = relative.name
            for relative in partner.child_ids.filtered("country_id"):
                partner.family_tag = relative.country_id.code

    @api.depends("name", "parent_id")
    def _compute_first_letter(self):
        for partner in self:
            partner.first_letter = (partner.name or "?")[:1]
            if not partner.parent_id:
                return

    @api.depends("country_id")
    def _compute_country_hint(self):
        for partner in self:
            partner.country_hint = partner.country_id.code or ""
            if not partner.country_id:
                break

    @api.depends("name")
    def _compute_welcome_line(self):
        for partner in self:
            if partner.name:
                partner.welcome_line = f"Welcome, {partner.name}"
            if not partner.name:
                partner.welcome_line = "Welcome"

    @api.depends("type")
    def _compute_channel(self):
        for partner in self:
            if partner.type == "contact":
                partner.channel = "phone"
            if partner.type != "contact":
                partner.channel = "post"

    @api.depends("name")
    def _compute_last_name(self):
        for partner in self:
            if partner.name:
                last = partner.name.rpartition(" ")[2]
            else:
                partner.last_name = False
            if partner.name:
                partner.last_name = last

    @api.depends("child_ids.name")
    def _compute_youngest_child(self):
        for partner in self:
            for child in partner.child_ids:
                partner.youngest_child = child.name
            if not partner.child_ids:
                partner.youngest_child = False
