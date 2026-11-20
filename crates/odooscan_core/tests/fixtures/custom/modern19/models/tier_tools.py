from functools import partial, partialmethod

from odoo import models


class ResPartnerTierTools(models.Model):
    _inherit = "res.partner"

    def _x_tier_text(self, prefix):
        return prefix + (self.tier or "")

    @classmethod
    def x_tier_count(cls):
        return 2

    @staticmethod
    def x_tier_scale(level):
        return level * 10

    x_tier_label = partial(str.format, "Tier {}")
    x_tier_mark = partialmethod(_x_tier_text, "* ")

    def x_tier_open(self):
        return True

    @classmethod
    def x_tier_kind(cls):
        return "loyalty"


class ResPartnerTierKind(models.Model):
    _inherit = "res.partner"

    def x_tier_kind(self):
        return self.tier or "none"
