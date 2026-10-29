import sys

import odoo
from odoo import fields, models
from odoo.addons.base.models.res_partner import ResPartner as BasePartner

from . import rollbacks


def _x_display_label(record):
    return record.display_name.upper()


def _x_short_label(record):
    return record.display_name[:8]


models.BaseModel._x_display_label = _x_display_label
BasePartner._x_short_label = _x_short_label
odoo.addons.x_partner_labels = sys.modules[__name__]
setattr(fields.Field, "x_label_width", 20)
models.BaseModel.__doc__ += "\nRecords also carry x labels."

if odoo.release.version_info[0] >= 18:
    fields.Char.x_label_case = "upper"

try:
    from odoo.tools import misc
except ImportError:
    misc = None
else:
    misc.x_label_separator = " / "

sys.modules["odoo.addons.x_labels"] = sys.modules[__name__]
rollbacks.LABEL_WIDTH = 30


def _x_patch_later():
    models.BaseModel._x_label_ready = True
