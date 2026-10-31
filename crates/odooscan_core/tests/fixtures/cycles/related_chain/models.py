from odoo import api, fields, models

class Link1(models.Model):
    _name = "chain.link1"

    next_id = fields.Many2one("chain.link2")
    end_id = fields.Many2one(related="next_id.end_id")
    good = fields.Char(related="end_id.name")
    parent_id = fields.Many2one("chain.link1")
    up_id = fields.Many2one(related="parent_id")
    twice_id = fields.Many2one(related="up_id.up_id")
    shown = fields.Char(compute="_compute_shown")

    @api.depends("end_id.name", "end_id.missing", "twice_id.missing")
    def _compute_shown(self):
        self.shown = False


class Link2(models.Model):
    _name = "chain.link2"

    next_id = fields.Many2one("chain.link3")
    end_id = fields.Many2one(related="next_id.end_id")


class Link3(models.Model):
    _name = "chain.link3"

    next_id = fields.Many2one("chain.link4")
    end_id = fields.Many2one(related="next_id.end_id")


class Link4(models.Model):
    _name = "chain.link4"

    next_id = fields.Many2one("chain.link5")
    end_id = fields.Many2one(related="next_id.end_id")


class Link5(models.Model):
    _name = "chain.link5"

    next_id = fields.Many2one("chain.link6")
    end_id = fields.Many2one(related="next_id.end_id")


class Link6(models.Model):
    _name = "chain.link6"

    next_id = fields.Many2one("chain.link7")
    end_id = fields.Many2one(related="next_id.end_id")


class Link7(models.Model):
    _name = "chain.link7"

    next_id = fields.Many2one("chain.link8")
    end_id = fields.Many2one(related="next_id.end_id")


class Link8(models.Model):
    _name = "chain.link8"

    next_id = fields.Many2one("chain.link9")
    end_id = fields.Many2one(related="next_id.end_id")


class Link9(models.Model):
    _name = "chain.link9"

    next_id = fields.Many2one("chain.link10")
    end_id = fields.Many2one(related="next_id.end_id")


class Link10(models.Model):
    _name = "chain.link10"

    next_id = fields.Many2one("chain.link11")
    end_id = fields.Many2one(related="next_id.end_id")


class Link11(models.Model):
    _name = "chain.link11"

    next_id = fields.Many2one("chain.link12")
    end_id = fields.Many2one(related="next_id.end_id")


class Link12(models.Model):
    _name = "chain.link12"

    end_id = fields.Many2one("res.partner")
