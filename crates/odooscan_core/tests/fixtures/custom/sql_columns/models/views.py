from odoo import fields, models, tools
from odoo.tools import SQL


class ParcelSummary(models.Model):
    _name = "parcel.summary"
    _description = "Parcel summary"
    _auto = False
    _sql_constraints = [("summary_positive", "CHECK(bogus_total > 0)", "Odoo applies no constraint to a view.")]

    depot_id = fields.Many2one("parcel.depot", readonly=True)
    item_count = fields.Integer(readonly=True)
    label = fields.Char(readonly=True)
    heaviest = fields.Float(readonly=True)
    snapshot = fields.Binary(readonly=True)
    share = fields.Float(compute="_compute_share")
    country_ids = fields.Many2many("res.country")
    handling_fee = fields.Float(company_dependent=True)

    def init(self):
        tools.drop_view_if_exists(self.env.cr, self._table)
        self.env.cr.execute(
            """
            CREATE OR REPLACE VIEW parcel_summary AS (
                SELECT b.id AS id, b.depot_id, count(i.id) AS item_count, b.name label
                  FROM parcel_box b
             LEFT JOIN parcel_item i ON i.box_id = b.id
              GROUP BY b.id
            )
            """
        )

    def _compute_share(self):
        for summary in self:
            summary.share = summary.item_count / 100.0

    def x_summary_lines(self):
        cr = self.env.cr
        cr.execute("SELECT s.label, s.item_count, s.heaviest FROM parcel_summary s")
        cr.execute("SELECT f.bogus FROM parcel_flat f")
        cr.execute("SELECT m.bogus FROM parcel_meter m")
        return cr.fetchall()


class ParcelSummaryVolume(models.Model):
    _inherit = "parcel.summary"

    volume = fields.Float(readonly=True)
    heaviest = fields.Float(readonly=True, digits=(16, 3))


class ParcelReportArchive(models.Model):
    _inherit = "parcel.report"
    _auto = False

    archived_count = fields.Integer()


class ParcelWeekly(models.Model):
    _name = "parcel.weekly"
    _description = "Parcel weekly count"
    _auto = False

    box_count = fields.Integer()

    def init(self):
        self.env.cr.execute(
            SQL(
                "CREATE OR REPLACE VIEW %s AS (SELECT count(*) AS box_count, date_trunc('week', create_date) AS week "
                "FROM parcel_box GROUP BY 2)",
                SQL.identifier(self._table),
            )
        )


class ParcelFlat(models.Model):
    _name = "parcel.flat"
    _description = "Parcel flat list"
    _auto = False

    weight = fields.Float()

    def init(self):
        self.env.cr.execute("CREATE OR REPLACE VIEW parcel_flat AS (SELECT * FROM parcel_item)")


class ParcelRoster(models.Model):
    _name = "parcel.roster"
    _description = "Parcel roster"
    _auto = False

    code = fields.Char()

    def init(self):
        self.env.cr.execute(
            "CREATE OR REPLACE VIEW parcel_roster AS (SELECT b.id AS id, %s FROM parcel_box b)" % self._roster_columns()
        )

    def _roster_columns(self):
        return "b.name AS code"


class ParcelLoad(models.Model):
    _name = "parcel.load"
    _description = "Parcel load"
    _auto = False

    load_total = fields.Float()

    def init(self):
        self.env.cr.execute(
            "CREATE OR REPLACE VIEW parcel_load AS (SELECT i.box_id AS id, sum(i.weight) FROM parcel_item i GROUP BY 1)"
        )


class ParcelPeriod(models.Model):
    _name = "parcel.period"
    _description = "Parcel period"
    _auto = False

    period_start = fields.Date()

    def init(self):
        self.env.cr.execute("CREATE OR REPLACE VIEW parcel_period AS (SELECT b.id AS id FROM parcel_box b)")


class ParcelPeriodRebuilt(models.Model):
    _inherit = "parcel.period"

    def init(self):
        self.env.cr.execute(
            "CREATE OR REPLACE VIEW parcel_period AS "
            "(SELECT b.id AS id, b.create_date::date AS period_start FROM parcel_box b)"
        )


class ParcelArchive(models.Model):
    _name = "parcel.archive"
    _description = "Parcel archive"
    _auto = False

    reference = fields.Char()

    def init(self):
        self.env.cr.execute("CREATE OR REPLACE VIEW parcel_archive_v1 AS (SELECT b.id AS id FROM parcel_box b)")


class ParcelSpan(models.Model):
    _name = "parcel.span"
    _description = "Parcel span"
    _auto = False

    span = fields.Integer()

    def init(self):
        self.env.cr.execute("CREATE OR REPLACE VIEW parcel_span (id, span) AS SELECT b.id, b.depot_id FROM parcel_box b")


class ParcelPeak(models.Model):
    _name = "parcel.peak"
    _description = "Parcel peak"
    _auto = False

    peak = fields.Float()

    def init(self):
        self.env.cr.execute(
            """
            CREATE OR REPLACE VIEW parcel_peak AS (
                WITH heavy AS (SELECT i.box_id, max(i.weight) AS top FROM parcel_item i GROUP BY i.box_id)
                SELECT h.box_id AS id, h.top AS peak FROM heavy h
            )
            """
        )
