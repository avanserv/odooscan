from odoo import fields, models


def related_kw(path):
    return {"related": path, "readonly": True}


class Rack(models.Model):
    _name = "x.rack"
    _description = "Rack"

    user_id = fields.Many2one("res.users")
    # The keywords hide the related, and may hide a store=True beside it, so the field may keep its
    # rows in any table, the one x.rack.tag reads among them.
    group_ids = fields.Many2many("res.groups", **related_kw("user_id.group_ids"))


class RackTag(models.Model):
    _name = "x.rack.tag"
    _description = "Rack tag link"
    _table = "x_rack_tag_rel"
    _auto = False

    x_rack_id = fields.Many2one("x.rack", readonly=True)
    res_groups_id = fields.Many2one("res.groups", readonly=True)
