import psycopg2
from psycopg2 import sql

from odoo import models
from odoo.tools import SQL


class ParcelBoxQueries(models.Model):
    _inherit = "parcel.box"

    def x_weight_units(self):
        query = "SELECT b.weight_uom FROM parcel_box b WHERE b.id IN %s"
        self.env.cr.execute(query, [tuple(self.ids)])
        return self.env.cr.fetchall()

    def x_box_labels(self):
        self.env.cr.execute("SELECT parcel_box.label FROM parcel_box")
        return self.env.cr.fetchall()

    def x_box_details(self):
        self.env.cr.execute(
            """
            SELECT b.total_weight, b.item_ids, b.tag_ids, l.name
              FROM parcel_box b
              JOIN parcel_locker l ON l.depot_id = b.depot_id
            """
        )
        return self.env.cr.fetchall()

    def x_full_depots(self):
        return self.env.execute_query(
            SQL(
                "SELECT d.id FROM parcel_depot d WHERE EXISTS "
                "(SELECT 1 FROM parcel_box b WHERE b.depot_id = d.id AND d.capacity > 0)"
            )
        )

    def x_upsert_box(self, name):
        self.env.cr.execute(
            "INSERT INTO parcel_box (name) VALUES (%s) ON CONFLICT (name) DO UPDATE SET name = EXCLUDED.title",
            [name],
        )
        self.env["parcel.box"].invalidate_model(["name"])

    def x_box_volumes(self):
        return self.env.execute_query(SQL("SELECT b.volume FROM parcel_box b WHERE b.id = %s", self.id))

    def x_known_columns(self):
        self.env.cr.execute(
            """
            SELECT b.id, b.create_uid, b.write_date, b.label_image, b.handling_fee, d.parent_path,
                   k.host_depot_id, l.code, b.preview_image, b.handling_note, b.pickup_code
              FROM parcel_box b
              JOIN parcel_depot d ON d.id = b.depot_id
              JOIN parcel_kiosk k ON k.host_depot_id = d.id
              JOIN parcel_locker l ON l.depot_id = d.id
             WHERE EXTRACT(year FROM b.create_date) = %s
               AND b.name IS DISTINCT FROM d.name
               FOR UPDATE
            """,
            [2024],
        )

    def x_unread_shapes(self):
        cr = self.env.cr
        cr.execute("WITH heavy AS (SELECT id FROM parcel_box) SELECT h.bogus FROM heavy h")
        cr.execute("SELECT d.bogus FROM (SELECT id FROM parcel_box) d")
        cr.execute("SELECT t.bogus FROM %s t" % self._table)
        cr.execute("SELECT t.bogus FROM {} t".format(self._table))
        cr.execute(SQL("SELECT b.id FROM parcel_box b WHERE %s", SQL("b.bogus = %s", 1)))
        cr.execute("SELECT b.perm_{} FROM parcel_box b".format("read"))
        cr.execute("SELECT public.parcel_box.bogus FROM parcel_box")
        cr.execute("SELECT b.bogus FROM parcel_box b WHERE b.name = 'open")

    def x_unread_tables(self):
        cr = self.env.cr
        cr.execute("SELECT r.bogus FROM parcel_report r")
        cr.execute("SELECT s.bogus FROM parcel_stat s")
        cr.execute("SELECT s.bogus FROM parcel_digest s")
        cr.execute("SELECT s.bogus FROM parcel_slot_a s")
        cr.execute("SELECT g.bogus FROM parcel_leg g")
        cr.execute("SELECT c.bogus FROM parcel_crate c")
        cr.execute("SELECT b.x_note FROM parcel_box b")
        cr.execute("SELECT p.x_rank FROM res_partner p")
        cr.execute("SELECT s.bogus FROM registry_shipment s")

    def x_rebound_query(self, heavy):
        query = "SELECT b.bogus FROM parcel_box b"
        if heavy:
            query = "SELECT b.other_bogus FROM parcel_box b"
        self.env.cr.execute(query)

    def x_extended_query(self):
        query = "SELECT b.bogus FROM parcel_box b"
        query += " WHERE b.id > 0"
        self.env.cr.execute(query)

    def x_box_titles(self):
        self.env.cr.execute("SELECT b.display_name FROM parcel_box b")
        return self.env.cr.fetchall()

    def x_box_codes_twice(self):
        query = "SELECT b.code FROM parcel_box b"
        self.env.cr.execute(query)
        first = self.env.cr.fetchall()
        self.env.cr.execute(query)
        return first == self.env.cr.fetchall()

    def x_legacy_boxes(self, dsn):
        cr = psycopg2.connect(dsn).cursor()
        cr.execute("SELECT b.legacy_code FROM parcel_box b")
        return cr.fetchall()

    def x_boxes_aside(self):
        with self.pool.cursor() as cr:
            cr.execute("SELECT b.shelf FROM parcel_box b")
            return cr.fetchall()

    def x_shelves_and_beacons(self):
        cr = self.env.cr
        cr.execute("SELECT s.code, s.occupancy FROM parcel_shelf s")
        cr.execute("SELECT c.id, c.signal, c.create_date FROM parcel_beacon c")
        cr.execute("SELECT u.login, u.password FROM res_users u")

    def x_box_seals(self):
        self.env.cr.execute("SELECT b.seal_code, b.seal_ref FROM parcel_box b")
        return self.env.cr.fetchall()

    def x_relays_and_antennas(self):
        cr = self.env.cr
        cr.execute("SELECT r.name, r.write_date FROM parcel_relay r")
        cr.execute("SELECT a.name, a.create_date FROM parcel_antenna a")
        cr.execute("SELECT b.name, b.__last_update FROM parcel_box b")

    def x_remote_boxes(self, remote):
        remote.execute(sql.SQL("SELECT b.remote_code FROM parcel_box b"))
        self.env.cr.execute(sql.SQL("SELECT b.local_code FROM parcel_box b"))
