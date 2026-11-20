from odoo import fields, models


class RefsThing(models.Model):
    _name = "refs.thing"
    _description = "Refs thing"


class RefsThingGo(models.Model):
    _inherit = "refs.thing"

    tag = fields.Char()

    def action_go(self):
        return super().action_go()

    def _compute_spare(self):
        return None

    def action_sign(self, stamp):
        return stamp


class RefsThingSign(models.Model):
    _inherit = "refs.thing"

    def action_sign(self):
        return None


class RefsHolder(models.Model):
    _name = "refs.holder"
    _description = "Refs holder"

    partner_id = fields.Many2one("res.partner")
    lantern_id = fields.Many2one("ghost.lantern")
    side_lantern_id = fields.Many2one("side.lantern")
    lantern_wick = fields.Char(related="lantern_id.wick")
    side_lantern_wick = fields.Char(related="side_lantern_id.wick")
    ghost_ids = fields.One2many("ghost.thing", "tag")
    refs_ids = fields.One2many("refs.thing", "tag")


# No indexed class introduces ghost.lantern, so a field it lacks here may come from its introducer.
class RefsChild(models.Model):
    _name = "refs.child"
    _description = "Refs child"
    _inherits = {"ghost.lantern": "lantern_id"}

    lantern_id = fields.Many2one("ghost.lantern", required=True, ondelete="cascade")
    flame = fields.Char(related="glow")


class RefsHeir(models.Model):
    _name = "refs.heir"
    _description = "Refs heir"
    _inherits = {"side.lantern": "lantern_id"}

    lantern_id = fields.Many2one("side.lantern", required=True, ondelete="cascade")
    flame = fields.Char(related="glow")


class RefsKid(models.Model):
    _name = "refs.kid"
    _description = "Refs kid"
    _inherit = ["ghost.lantern"]

    flame = fields.Char(related="glow")


class RefsCub(models.Model):
    _name = "refs.cub"
    _description = "Refs cub"
    _inherit = ["side.lantern"]

    flame = fields.Char(related="glow")


class RefsLookup(models.AbstractModel):
    _name = "refs.lookup"
    _description = "Refs lookup"

    def _lanterns(self):
        return self.env["ghost.lantern"], self.env["side.lantern"], self.env["ghost.thing"]


# Only ghost_refs_side defines wick here. Through ghost.lantern, whose introducer may give it too,
# it reads as unknown; through side.lantern it comes from a module outside depends.
class RefsChildWick(models.Model):
    _inherit = "refs.child"

    wick_copy = fields.Char(related="wick")


class RefsKidWick(models.Model):
    _inherit = "refs.kid"

    wick_copy = fields.Char(related="wick")


class RefsHeirWick(models.Model):
    _inherit = "refs.heir"

    wick_copy = fields.Char(related="wick")


class RefsCubWick(models.Model):
    _inherit = "refs.cub"

    wick_copy = fields.Char(related="wick")


class RefsMid(models.Model):
    _name = "refs.mid"
    _description = "Refs mid"
    _inherit = ["ghost.lantern"]


class RefsGrand(models.Model):
    _name = "refs.grand"
    _description = "Refs grand"
    _inherits = {"refs.mid": "mid_id"}

    mid_id = fields.Many2one("refs.mid", required=True, ondelete="cascade")
    wick_copy = fields.Char(related="wick")


class RefsHolderWick(models.Model):
    _inherit = "refs.holder"

    kid_id = fields.Many2one("refs.kid", domain="[('wick', '=', 'lit')]")
    cub_id = fields.Many2one("refs.cub", domain="[('wick', '=', 'lit')]")
