from odoo import fields, models


class Base(models.AbstractModel):
    _name = "base"

    def _get_default_calendar_view(self):
        return None


class ResCountry(models.Model):
    _name = "res.country"

    name = fields.Char()
    code = fields.Char()


class ResCurrency(models.Model):
    _name = "res.currency"

    name = fields.Char()


class ResCompany(models.Model):
    _name = "res.company"

    name = fields.Char()


class ResPartner(models.Model):
    _name = "res.partner"

    name: str = fields.Char()
    type = fields.Selection([("contact", "Contact"), ("invoice", "Invoice")], required=True, default="contact")
    country_id = fields.Many2one("res.country")
    parent_id = fields.Many2one("res.partner")
    child_ids = fields.One2many("res.partner", "parent_id")
    short_name = fields.Char(compute="_compute_short_name", store=True, readonly=False)

    def _compute_short_name(self):
        for partner in self:
            partner.short_name = (partner.name or "")[:12]


class ResUsers(models.Model):
    _name = "res.users"

    login = fields.Char()
    password = fields.Char(compute="_x_blank_secret", inverse="_x_keep_secret")

    def _x_blank_secret(self):
        self.update({"password": None})

    def _x_keep_secret(self):
        pass


class ResGroups(models.Model):
    _name = "res.groups"

    name = fields.Char()


class IrUiView(models.Model):
    _name = "ir.ui.view"

    name = fields.Char()
    model = fields.Char()
    type = fields.Char()
    key = fields.Char()
    arch = fields.Text()
    inherit_id = fields.Many2one("ir.ui.view")


class IrActionsActWindow(models.Model):
    _name = "ir.actions.act_window"

    name = fields.Char()
    view_mode = fields.Char()
    res_model = fields.Char()
    domain = fields.Char()
    context = fields.Char()
    view_id = fields.Many2one("ir.ui.view")
    view_ids = fields.One2many("ir.actions.act_window.view", "act_window_id")


class IrActionsActWindowView(models.Model):
    _name = "ir.actions.act_window.view"

    view_mode = fields.Char()
    view_id = fields.Many2one("ir.ui.view")
    act_window_id = fields.Many2one("ir.actions.act_window")


class IrModelAccess(models.Model):
    _name = "ir.model.access"

    name = fields.Char()
    active = fields.Boolean()
    model_id = fields.Many2one("ir.model")
    group_id = fields.Many2one("res.groups")
    perm_read = fields.Boolean()
    perm_write = fields.Boolean()
    perm_create = fields.Boolean()
    perm_unlink = fields.Boolean()


class IrRule(models.Model):
    _name = "ir.rule"

    name = fields.Char()
    active = fields.Boolean()
    model_id = fields.Many2one("ir.model")
    groups = fields.Many2many("res.groups")
    domain_force = fields.Text()


class IrAccess(models.Model):
    _name = "ir.access"

    name = fields.Char()
    active = fields.Boolean()
    model_id = fields.Many2one("ir.model")
    group_id = fields.Many2one("res.groups")
    operation = fields.Char()
    domain = fields.Char()


class IrModel(models.Model):
    _name = "ir.model"

    model = fields.Char()
    rule_ids = fields.One2many("ir.rule", "model_id")


class IrHttp(models.AbstractModel):
    _name = "ir.http"

    @classmethod
    def _handle_error(cls, exception):
        return exception
