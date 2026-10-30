from odoo import fields

from . import controllers, models


def _widen_labels():
    fields.Char.x_label_width = 40
