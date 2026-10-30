from odoo import fields, models

COUNTER_PARENTS = {"parcel.depot": "x_counter_depot_id"}
MOORING_EXTRA = "parcel.boxed"
SLIP_PARENTS = ["parcel.slip"]


class ParcelHosted(models.AbstractModel):
    _name = "parcel.hosted"
    _description = "Parcel hosted"
    _inherits = {"parcel.depot": "x_host_id"}


class ParcelSited(models.AbstractModel):
    _name = "parcel.sited"
    _description = "Parcel sited"
    _inherits = {"parcel.depot": "x_site_id"}


class ParcelStand(models.Model):
    _name = "parcel.stand"
    _inherit = ["parcel.hosted"]
    _description = "Parcel stand"
    _sql_constraints = [
        ("host_unique", "UNIQUE(x_host_id)", "A depot hosts one stand."),
        ("name_unique", "UNIQUE(name)", "Stand names are unique."),
    ]

    def x_stand_columns(self):
        self.env.cr.execute("SELECT s.x_host_id, s.name, s.bogus FROM parcel_stand s")
        return self.env.cr.fetchall()


class ParcelBooth(models.Model):
    _name = "parcel.booth"
    _inherit = ["parcel.hosted"]
    _description = "Parcel booth"
    _inherits = {"parcel.depot": "x_old_id"}


class ParcelBoothDepot(models.Model):
    _inherit = "parcel.booth"
    _inherits = {"parcel.depot": "x_booth_depot_id"}

    def x_booth_columns(self):
        self.env.cr.execute("SELECT b.x_host_id, b.x_old_id, b.x_booth_depot_id, b.name FROM parcel_booth b")
        return self.env.cr.fetchall()


class ParcelCounter(models.Model):
    _name = "parcel.counter"
    _inherit = ["parcel.hosted"]
    _description = "Parcel counter"
    _inherits = COUNTER_PARENTS

    def x_counter_columns(self):
        self.env.cr.execute("SELECT c.bogus FROM parcel_counter c")
        return self.env.cr.fetchall()


class ParcelPost(models.Model):
    _name = "parcel.post"
    _inherit = ["parcel.hosted", "parcel.sited"]
    _description = "Parcel post"
    _sql_constraints = [
        ("host_unique", "UNIQUE(x_host_id)", "A depot hosts one post."),
        ("site_unique", "UNIQUE(x_site_id)", "A depot sites one post."),
    ]

    def x_post_columns(self):
        self.env.cr.execute("SELECT p.x_host_id, p.x_site_id, p.name FROM parcel_post p")
        return self.env.cr.fetchall()


class ParcelBoxed(models.AbstractModel):
    _name = "parcel.boxed"
    _description = "Parcel boxed"
    _inherits = {"parcel.box": "x_boxed_id"}


# Entries for different parent models both count, whichever parent Odoo merges first.
class ParcelJetty(models.Model):
    _name = "parcel.jetty"
    _inherit = ["parcel.hosted", "parcel.boxed"]
    _description = "Parcel jetty"

    def x_jetty_columns(self):
        self.env.cr.execute("SELECT j.x_host_id, j.x_boxed_id FROM parcel_jetty j")
        return self.env.cr.fetchall()


class ParcelRehosted(models.AbstractModel):
    _name = "parcel.rehosted"
    _inherit = ["parcel.hosted"]
    _description = "Parcel rehosted"
    _inherits = {"parcel.depot": "x_rehost_id"}


class ParcelWharf(models.Model):
    _name = "parcel.wharf"
    _inherit = ["parcel.rehosted", "parcel.sited"]
    _description = "Parcel wharf"

    def x_wharf_columns(self):
        self.env.cr.execute("SELECT w.x_rehost_id, w.x_host_id, w.x_site_id FROM parcel_wharf w")
        return self.env.cr.fetchall()


class ParcelQuay(models.Model):
    _name = "parcel.quay"
    _inherit = ["parcel.sited", "parcel.rehosted", "parcel.hosted"]
    _description = "Parcel quay"

    def x_quay_columns(self):
        self.env.cr.execute("SELECT q.x_site_id, q.x_host_id FROM parcel_quay q")
        return self.env.cr.fetchall()


class ParcelMooring(models.Model):
    _name = "parcel.mooring"
    _inherit = ["parcel.hosted", MOORING_EXTRA, "parcel.sited"]
    _description = "Parcel mooring"

    def x_mooring_columns(self):
        self.env.cr.execute("SELECT m.x_host_id, m.x_site_id FROM parcel_mooring m")
        return self.env.cr.fetchall()


class ParcelKeel(models.AbstractModel):
    _name = "parcel.keel"
    _description = "Parcel keel"


class ParcelHull(models.AbstractModel):
    _name = "parcel.hull"
    _description = "Parcel hull"

    x_draft = fields.Char(compute="_compute_x_draft", store=False)

    def _compute_x_draft(self):
        for hull in self:
            hull.x_draft = hull.display_name


class ParcelDeck(models.AbstractModel):
    _name = "parcel.deck"
    _inherit = ["parcel.keel", "parcel.hull"]
    _description = "Parcel deck"


class ParcelMast(models.AbstractModel):
    _name = "parcel.mast"
    _inherit = ["parcel.keel"]
    _description = "Parcel mast"

    x_draft = fields.Char(compute="_compute_x_draft", store=True)

    def _compute_x_draft(self):
        for mast in self:
            mast.x_draft = mast.display_name


# Field definitions of two parents stay unordered: as both merge parcel.keel, Python's method
# resolution order merges parcel.mast after parcel.hull, against the order of the parents.
class ParcelFerry(models.Model):
    _name = "parcel.ferry"
    _inherit = ["parcel.deck", "parcel.mast"]
    _description = "Parcel ferry"

    def x_ferry_columns(self):
        self.env.cr.execute("SELECT f.x_draft FROM parcel_ferry f")
        return self.env.cr.fetchall()


class ParcelGantry(models.AbstractModel):
    _name = "parcel.gantry"
    _description = "Parcel gantry"

    x_lift = fields.Char(compute="_compute_x_lift", store=False)

    def _compute_x_lift(self):
        for gantry in self:
            gantry.x_lift = gantry.display_name


class ParcelCrane(models.Model):
    _name = "parcel.crane"
    _inherit = ["parcel.gantry"]
    _description = "Parcel crane"


class ParcelHoist(models.Model):
    _name = "parcel.hoist"
    _description = "Parcel hoist"

    x_lift = fields.Char(compute="_compute_x_lift", store=True)

    def _compute_x_lift(self):
        for hoist in self:
            hoist.x_lift = hoist.display_name


# The parents share no model, yet Python's method resolution order merges parcel.gantry before
# parcel.hoist, so the stored definition wins against the order of the parents.
class ParcelYard(models.Model):
    _name = "parcel.yard"
    _inherit = ["parcel.crane", "parcel.hoist"]
    _description = "Parcel yard"

    def x_yard_columns(self):
        self.env.cr.execute("SELECT y.x_lift FROM parcel_yard y")
        return self.env.cr.fetchall()


class ParcelLanding(models.Model):
    _name = "parcel.landing"
    _inherit = ["parcel.keel", "parcel.hull", "parcel.hosted"]
    _description = "Parcel landing"


# Odoo adds parcel.sited after the classes of parcel.landing in the bases, so it merges before
# parcel.hosted, whose entry wins. Two classes list the parents, so the scan leaves their order
# unknown and both names stay silent. Without _name, listing two models would make a new model.
class ParcelLandingSited(models.Model):
    _name = "parcel.landing"
    _inherit = ["parcel.landing", "parcel.sited"]

    def x_landing_columns(self):
        self.env.cr.execute("SELECT l.x_host_id, l.x_site_id FROM parcel_landing l")
        return self.env.cr.fetchall()


class ParcelBollard(models.AbstractModel):
    _name = "parcel.bollard"
    _inherit = ["parcel.keel"]
    _description = "Parcel bollard"
    _inherits = {"parcel.depot": "x_bollard_id"}


class ParcelCleat(models.AbstractModel):
    _name = "parcel.cleat"
    _inherit = ["parcel.keel"]
    _description = "Parcel cleat"
    _inherits = {"parcel.depot": "x_cleat_id"}


# Odoo merges the _inherits of each parent whole, so a model both share leaves their order alone.
class ParcelPontoon(models.Model):
    _name = "parcel.pontoon"
    _inherit = ["parcel.bollard", "parcel.cleat"]
    _description = "Parcel pontoon"

    def x_pontoon_columns(self):
        self.env.cr.execute("SELECT p.x_bollard_id, p.x_cleat_id FROM parcel_pontoon p")
        return self.env.cr.fetchall()


class ParcelSlip(models.AbstractModel):
    _name = "parcel.slip"
    _description = "Parcel slip"
    _inherits = {"parcel.depot": "x_slip_id"}


class ParcelMoored(models.AbstractModel):
    _name = "parcel.moored"
    _inherit = SLIP_PARENTS
    _description = "Parcel moored"


class ParcelTethered(models.AbstractModel):
    _name = "parcel.tethered"
    _inherit = ["parcel.slip"]
    _description = "Parcel tethered"
    _inherits = {"parcel.depot": "x_tether_id"}


# parcel.moored may reach parcel.slip too, leaving unknown which entry Odoo keeps.
class ParcelMarina(models.Model):
    _name = "parcel.marina"
    _inherit = ["parcel.moored", "parcel.tethered"]
    _description = "Parcel marina"

    def x_marina_columns(self):
        self.env.cr.execute("SELECT m.x_slip_id FROM parcel_marina m")
        return self.env.cr.fetchall()


class ParcelTideBase(models.AbstractModel):
    _inherit = "base"

    x_tide = fields.Integer(compute="_compute_x_tide")

    def _compute_x_tide(self):
        for record in self:
            record.x_tide = record.id


class ParcelTidal(models.AbstractModel):
    _name = "parcel.tidal"
    _description = "Parcel tidal"

    x_tide = fields.Char(compute="_compute_x_tide")

    def _compute_x_tide(self):
        for tidal in self:
            tidal.x_tide = tidal.display_name


# A class of base is left unordered against a parent's, so this stays silent.
class ParcelLagoon(models.Model):
    _name = "parcel.lagoon"
    _inherit = ["parcel.tidal"]
    _description = "Parcel lagoon"

    def x_lagoon_columns(self):
        self.env.cr.execute("SELECT l.x_tide FROM parcel_lagoon l")
        return self.env.cr.fetchall()


class ParcelShop(models.Model):
    _name = "parcel.shop"
    _description = "Parcel shop"
    _inherits = {"parcel.depot": "x_shop_depot_id"}

    x_box_id = fields.Many2one("parcel.box")
    x_shop_depot_id = fields.Many2one(
        "parcel.depot", related="x_box_id.depot_id", required=True, ondelete="cascade"
    )

    def x_shop_columns(self):
        self.env.cr.execute("SELECT s.x_shop_depot_id FROM parcel_shop s")
        return self.env.cr.fetchall()


class ParcelAnnex(models.Model):
    _name = "parcel.annex"
    _description = "Parcel annex"

    x_annex_box_id = fields.Many2one("parcel.box", delegate=True, required=True, ondelete="cascade")

    def x_annex_columns(self):
        self.env.cr.execute("SELECT a.name FROM parcel_annex a")
        return self.env.cr.fetchall()


class ParcelOutlet(models.Model):
    _name = "parcel.outlet"
    _description = "Parcel outlet"

    x_outlet_box_id = fields.Many2one("parcel.box", delegate=True, required=True, ondelete="cascade")
    _inherits = {"parcel.depot": "x_outlet_depot_id"}

    def x_outlet_columns(self):
        self.env.cr.execute("SELECT o.name FROM parcel_outlet o")
        return self.env.cr.fetchall()
