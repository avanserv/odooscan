from collections import defaultdict

from odoo import models
from odoo.tools.lru import LRU

_label_by_partner = {}
_seen_codes = set()
_visits_by_day = defaultdict(int)
_recent_labels = LRU(16)
_lower_codes = {code: code.lower() for code in ("BE", "FR")}
_fallback_by_db = {}
COUNTRY_PREFIXES = {"BE": "32", "FR": "33"}
_warmed_codes = []
_warmed_codes.append("BE")
_scratch = {}


class ResPartner(models.Model):
    _inherit = "res.partner"

    def _x_label(self):
        if self.id not in _label_by_partner:
            _label_by_partner[self.id] = self.display_name
        return _label_by_partner[self.id]

    def _x_mark_seen(self, code):
        _seen_codes.add(code)

    def _x_forget_day(self, day):
        del _visits_by_day[day]

    def _x_remember(self):
        _recent_labels[self.id] = self.display_name

    def _x_add_code(self, code):
        _lower_codes.update({code: code.lower()})

    def _x_fallback(self):
        _fallback_by_db[self.env.cr.dbname] = self.display_name
        return _fallback_by_db[self.env.cr.dbname]

    def _x_forget_fallback(self):
        _fallback_by_db.pop(self.env.cr.dbname, None)

    def _x_fallback_of(self):
        _fallback_by_db[(self.env.cr.dbname, self.id)] = self.display_name

    def _x_prefix(self, code):
        return COUNTRY_PREFIXES.get(code, "")

    def _x_scratch(self):
        _scratch = {}
        _scratch["last"] = self.id
        return _scratch
