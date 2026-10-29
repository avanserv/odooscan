from odoo import fields, models

COUNTER_PARENTS = {"parcel.depot": "x_counter_depot_id"}


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

    def x_post_columns(self):
        self.env.cr.execute("SELECT p.x_host_id, p.x_site_id FROM parcel_post p")
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
