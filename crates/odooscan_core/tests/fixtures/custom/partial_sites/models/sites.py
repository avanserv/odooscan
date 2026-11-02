from odoo import api, fields, models
from odoo.addons.ghost_enterprise.models.layout import Layout


class SiteTask(models.Model):
    _inherit = "project.task"
    _order = "x_ent"

    project_id = fields.Many2one("project.project", domain=[("x_ent", "=", 1)])
    total = fields.Float(compute="_compute_from_enterprise")
    rate = fields.Float(compute="_compute_rate")

    @api.depends("x_ent")
    def _compute_rate(self):
        self.rate = 0


class SitePartner(models.Model):
    _inherit = "res.partner"

    def action_sites(self):
        self.env["res.partner"].search([("x_ent", "=", 1)])
        self.env.ref("mail.mail_menu")
        self.env.ref("base.no_such_id")
        self.env.ref("mail.no_such_id")
        self.env.ref("encoded_lost.no_such_id")
        self.env.cr.execute("SELECT p.x_bogus FROM res_partner p")

    def _prepare_from_missing_module(self):
        return super()._prepare_from_missing_module()

    @api.depends("name.x")
    def _compute_not_relational(self):
        self.total = 0


class SiteCountry(models.Model):
    _name = "res.country"
    _inherit = _name

    fee = fields.Monetary()


class SiteVisit(models.Model):
    _name = "site.visit"
    _description = "Site visit"
    _order = "nmae"

    name = fields.Char()
    partner_id = fields.Many2one("res.partner")
    line_ids = fields.One2many("site.visit.line", "visit_id", domain=[("nmae", "=", 1)])
    total = fields.Float(compute="_compute_typo")
    rate = fields.Float(compute="_compute_rate")

    @api.depends("nmae", "line_ids.nmae", "partner_id.x_ent", "name.x")
    def _compute_rate(self):
        self.rate = 0

    def action_lines(self):
        self.env["site.visit"].search([("nmae", "=", 1)])


class SiteVisitLine(models.Model):
    _name = "site.visit.line"
    _description = "Site visit line"

    visit_id = fields.Many2one("site.visit")


class SiteThread(models.Model):
    _name = "site.thread"
    _description = "Site thread"
    _inherit = ["mail.thread"]
    _order = "x_ent"

    total = fields.Float(compute="_compute_from_enterprise")


class SiteSheet(Layout, models.Model):
    _name = "site.sheet"
    _description = "Site sheet"
    _order = "x_ent"
