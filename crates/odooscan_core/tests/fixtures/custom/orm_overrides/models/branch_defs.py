import contextlib

from odoo import fields, models

BRANCH_HINTS = True


class ResPartnerBranchComputes(models.Model):
    _inherit = "res.partner"

    branch_nickname = fields.Char(compute="_compute_branch_nickname")
    branch_motto = fields.Char(compute="_compute_branch_motto")
    branch_crest = fields.Char(compute="_compute_branch_crest")
    branch_label = fields.Char(compute="_compute_branch_label")

    if BRANCH_HINTS:

        def _compute_branch_nickname(self):
            for partner in self:
                if partner.ref:
                    partner.branch_nickname = partner.ref

    def _compute_branch_motto(self):
        for partner in self:
            if partner.ref:
                partner.branch_motto = partner.ref

    if BRANCH_HINTS:

        def _compute_branch_motto(self):
            for partner in self:
                partner.branch_motto = partner.ref or False

    def _compute_branch_crest(self):
        for partner in self:
            if partner.is_company:
                partner.branch_crest = partner.name

    if BRANCH_HINTS:

        def _compute_branch_label(self):
            for partner in self:
                partner.branch_label = partner.ref or False

    def _compute_branch_label(self):
        for partner in self:
            if partner.ref:
                partner.branch_label = partner.ref


class ResPartnerBranchCrest(models.Model):
    _inherit = "res.partner"

    try:
        crest_width = int(BRANCH_HINTS)
    except TypeError:
        crest_width = 0
    else:

        def _compute_branch_crest(self):
            super()._compute_branch_crest()
            for partner in self.filtered(lambda p: not p.is_company):
                partner.branch_crest = False


class ResPartnerBranchSql(models.Model):
    _inherit = "res.partner"

    def _reset_branch_colors(self):
        self.env.cr.execute("UPDATE res_partner SET color = 0 WHERE id IN %s", [tuple(self.ids)])
        self._forget_branch_colors()

    try:
        color_limit = int(BRANCH_HINTS)
    except TypeError:
        color_limit = 0
    else:

        def _forget_branch_colors(self):
            self.invalidate_model(["color"])

    with contextlib.suppress(AttributeError):

        def _clear_branch_colors(self):
            self.env.cr.execute("UPDATE res_partner SET color = 0")


class ResPartnerBranchGroups(models.Model):
    _inherit = "res.partner"

    if BRANCH_HINTS:

        def user_has_groups(self, groups):
            return self.env.user.has_groups(groups)

    def _branch_is_admin(self):
        return self.user_has_groups("base.group_system")


class ResCompanyBranchGroups(models.Model):
    _inherit = "res.company"

    if BRANCH_HINTS:

        def user_has_groups(self, groups):
            return super().user_has_groups(groups)

    def _branch_is_manager(self):
        return self.user_has_groups("base.group_erp_manager")
