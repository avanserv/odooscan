import sys

from odoo import fields, models

LOOP_VARIANTS = ("plain",)


class ResPartnerLoopComputes(models.Model):
    _inherit = "res.partner"

    loop_nickname = fields.Char(compute="_compute_loop_nickname")
    loop_badge = fields.Char(compute="_compute_loop_badge")
    loop_motto = fields.Char(compute="_compute_loop_motto")
    loop_crest = fields.Char(compute="_compute_loop_crest")
    loop_label = fields.Char(compute="_compute_loop_label")
    loop_ribbon = fields.Char(compute="_compute_loop_ribbon")

    def _compute_loop_nickname(self):
        for partner in self:
            if partner.ref:
                partner.loop_nickname = partner.ref

    match sys.version_info:
        case (3, *_):

            def _compute_loop_nickname(self):
                for partner in self:
                    partner.loop_nickname = partner.ref or False

            def _compute_loop_badge(self):
                for partner in self:
                    partner.loop_badge = partner.ref or False

    def _compute_loop_motto(self):
        for partner in self:
            if partner.ref:
                partner.loop_motto = partner.ref

    for _variant in LOOP_VARIANTS:

        def _compute_loop_motto(self):
            for partner in self:
                partner.loop_motto = partner.ref or False

    def _compute_loop_crest(self):
        for partner in self:
            if partner.ref:
                partner.loop_crest = partner.ref

    _attempts = 0
    while _attempts < len(LOOP_VARIANTS):
        _attempts += 1
    else:

        def _compute_loop_crest(self):
            for partner in self:
                partner.loop_crest = partner.ref or False

    for _variant in LOOP_VARIANTS:

        def _compute_loop_label(self):
            for partner in self:
                partner.loop_label = partner.ref or False

    def _compute_loop_label(self):
        for partner in self:
            if partner.ref:
                partner.loop_label = partner.ref

    def _compute_loop_ribbon(self):
        for partner in self:
            if partner.is_company:
                partner.loop_ribbon = partner.name


class ResPartnerLoopRibbon(models.Model):
    _inherit = "res.partner"

    match sys.version_info:
        case (3, *_):

            def _compute_loop_ribbon(self):
                super()._compute_loop_ribbon()
                for partner in self.filtered(lambda p: not p.is_company):
                    partner.loop_ribbon = False


class ResPartnerLoopSql(models.Model):
    _inherit = "res.partner"

    def _reset_loop_colors(self):
        self.env.cr.execute("UPDATE res_partner SET color = 0 WHERE id IN %s", [tuple(self.ids)])
        self._forget_loop_colors()

    for _variant in LOOP_VARIANTS:

        def _forget_loop_colors(self):
            self.invalidate_model(["color"])

        def _clear_loop_colors(self):
            self.env.cr.execute("UPDATE res_partner SET color = 0")


class ResUsersLoopGroups(models.Model):
    _inherit = "res.users"

    for _variant in LOOP_VARIANTS:

        def user_has_groups(self, groups):
            return self.env.user.has_groups(groups)

    def _loop_is_admin(self):
        return self.user_has_groups("base.group_system")


class ResCountryLoopGroups(models.Model):
    _inherit = "res.country"

    match sys.version_info:
        case (3, *_):

            def user_has_groups(self, groups):
                return super().user_has_groups(groups)

    def _loop_is_manager(self):
        return self.user_has_groups("base.group_erp_manager")
