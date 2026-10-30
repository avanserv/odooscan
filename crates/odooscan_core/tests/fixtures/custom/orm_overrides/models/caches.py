from odoo import models, tools


class ResPartner(models.Model):
    _inherit = "res.partner"

    @tools.ormcache("lang")
    def _greeting_for(self, lang_code):
        return lang_code.split("_")[0]

    @tools.ormcache_context("country", keys=("lang",))
    def _country_label(self, country_code):
        return country_code.upper()

    @tools.ormcache("self.env.uid", "frozenset(codes)", "tuple(code for code in codes)", "args")
    def _codes_for_user(self, codes, *args):
        return sorted(codes)

    @tools.ormcache("label_code", skiparg=1)
    def _legacy_label(self, code):
        return code.title()

    @tools.ormcache("self.env.context.get('lang'")
    def _broken_key(self):
        return self.env.lang

    @tools.ormcache("int(time.monotonic() // 900)")
    def _quarter_hour_rates(self):
        return {}

    @tools.ormcache("method.__name__")
    def _named_by_method(self):
        return self._name
