from odoo import fields, models


class LevelTop(models.Model):
    _name = "level.top"
    _description = "Level top"

    holder = fields.Many2one("res.partner")
    rank = fields.Integer()


class LevelMiddle(models.Model):
    _name = "level.middle"
    _description = "Level middle"
    _inherits = {"level.top": "top_id"}

    top_id = fields.Many2one("level.top", required=True, ondelete="cascade")
    holder = fields.Char()
    rank = fields.Char()
    rank_number = fields.Integer(related="rank")
    rank_label = fields.Char(related="rank")


class LevelBottom(models.Model):
    _name = "level.bottom"
    _description = "Level bottom"
    _inherits = {"level.middle": "middle_id"}

    middle_id = fields.Many2one("level.middle", required=True, ondelete="cascade")
    holder_name = fields.Char(related="holder.name")
    rank_number = fields.Integer(related="rank")
    middle_rank = fields.Integer(related="middle_id.rank")


class LevelBridge(models.Model):
    _name = "level.bridge"
    _description = "Level bridge"
    _inherits = {"level.top": "top_id"}

    top_id = fields.Many2one("level.top", required=True, ondelete="cascade")


class LevelThrough(models.Model):
    _name = "level.through"
    _description = "Level through"
    _inherits = {"level.bridge": "bridge_id"}

    bridge_id = fields.Many2one("level.bridge", required=True, ondelete="cascade")
    holder_name = fields.Char(related="holder.name")
    rank_number = fields.Integer(related="rank")
