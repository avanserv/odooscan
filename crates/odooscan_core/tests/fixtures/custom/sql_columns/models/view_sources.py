from odoo import fields, models, tools
from odoo.tools import SQL


class ParcelTally(models.Model):
    _name = "parcel.tally"
    _description = "Parcel tally"
    _auto = False
    _table_query = "SELECT 1 AS id, 0 AS box_count"

    box_count = fields.Integer()


class ParcelGauge(models.Model):
    _name = "parcel.gauge"
    _description = "Parcel gauge"
    _auto = False
    _table_query = "SELECT 1 AS id, 0.0 AS level"

    level = fields.Float()

    def init(self):
        self.env.cr.execute("CREATE OR REPLACE VIEW parcel_gauge AS (SELECT 1 AS id)")


class ParcelStatRules(models.Model):
    _inherit = "parcel.stat"
    _sql_constraints = [("stat_positive", "CHECK(bogus_count >= 0)", "Odoo applies no constraint to a table query.")]


class ParcelCensus(models.Model):
    _name = "parcel.census"
    _description = "Parcel census"
    _auto = False

    box_count = fields.Integer()

    @property
    def _table_sql(self):
        return SQL("SELECT 1 AS id, 0 AS box_count")


class ParcelMeter(models.Model):
    _name = "parcel.meter"
    _description = "Parcel meter"

    reading = fields.Integer()

    @property
    def _table_sql(self):
        return SQL("SELECT 1 AS id, 0 AS reading")


class ParcelViewMixin(models.AbstractModel):
    _name = "parcel.view.mixin"
    _description = "Parcel view mixin"

    def init(self):
        tools.drop_view_if_exists(self.env.cr, self._table)
        self.env.cr.execute(SQL("CREATE VIEW %s AS (SELECT 1 AS id)", SQL.identifier(self._table)))


class ParcelLedger(models.Model):
    _name = "parcel.ledger"
    _inherit = ["parcel.view.mixin"]
    _description = "Parcel ledger"
    _auto = False

    entry_count = fields.Integer()


class ParcelJournal(models.Model):
    _name = "parcel.journal"
    _description = "Parcel journal"
    _auto = False

    line_count = fields.Integer()

    def _auto_init(self):
        result = super()._auto_init()
        self.env.cr.execute(
            SQL("CREATE OR REPLACE VIEW %s AS (SELECT 1 AS id, 0 AS line_count)", SQL.identifier(self._table))
        )
        return result


class ParcelViewBase(models.AbstractModel):
    _name = "parcel.view.base"
    _description = "Parcel view base"
    _auto = False


class ParcelBoard(models.Model):
    _name = "parcel.board"
    _description = "Parcel board"
    _auto = False

    board_count = fields.Integer()


class ParcelSheetLayout:
    def _x_sheet_title(self):
        return "Sheet"


class ParcelSheet(models.Model, ParcelSheetLayout):
    _name = "parcel.sheet"
    _description = "Parcel sheet"
    _auto = False

    title = fields.Char()


class ParcelLegView(models.Model):
    _name = "parcel.leg.view"
    _description = "Parcel leg view"
    _table = "parcel_leg"
    _auto = False

    name = fields.Char()


class ParcelManifest(models.Model):
    _name = "parcel.manifest"
    _description = "Parcel manifest"
    _auto = False

    line_count = fields.Integer()
    weight_total = fields.Float()


class ParcelWaybill(models.Model):
    _name = "parcel.waybill"
    _description = "Parcel waybill"
    _auto = False

    stop_count = fields.Integer()


class ParcelDial(models.Model):
    _name = "parcel.dial"
    _description = "Parcel dial"
    _auto = False
    _table_sql = SQL("SELECT 1 AS id, 0 AS turn_count")

    turn_count = fields.Integer()


class ParcelDock(models.Model):
    _name = "parcel.dock"
    _description = "Parcel dock"
    _auto = False

    bay_count = fields.Integer()


class ParcelBay(models.Model):
    _name = "parcel.bay"
    _description = "Parcel bay"
    _auto = False

    door_count = fields.Integer()


class ParcelPier(models.Model):
    _name = "parcel.pier"
    _description = "Parcel pier"
    _auto = False

    berth_count = fields.Integer()


class ParcelBerth(models.Model):
    _name = "parcel.berth"
    _description = "Parcel berth"
    _auto = False

    crane_count = fields.Integer()


class ParcelViewTemplate(models.Model):
    _name = "parcel.view.template"
    _description = "Parcel view template"
    _register = False
    _auto = False

    shown_count = fields.Integer()


class ParcelStampBase(models.Model):
    _name = "parcel.stamp.base"
    _description = "Parcel stamp template"
    _register = False

    code = fields.Char()


class ParcelStamp(ParcelStampBase):
    _name = "parcel.stamp"
    _description = "Parcel stamp"


class ParcelBoxCountry(models.Model):
    _name = "parcel.box.country"
    _description = "Parcel box country link"
    _table = "parcel_box_res_country_rel"
    _auto = False

    parcel_box_id = fields.Many2one("parcel.box", readonly=True)
    res_country_id = fields.Many2one("res.country", readonly=True)


class ParcelTrip(models.Model):
    _name = "parcel.trip"
    _description = "Parcel trip"
    _auto = False

    leg_count = fields.Integer()

    def _create_view(self):
        tools.drop_view_if_exists(self.env.cr, self._table)
        self.env.cr.execute(
            SQL("CREATE VIEW %s AS (SELECT 1 AS id, 0 AS leg_count)", SQL.identifier(self._table))
        )


class ParcelRackViewBuilder:
    def init(self):
        self.env.cr.execute(
            SQL("CREATE OR REPLACE VIEW %s AS (SELECT 1 AS id, 0 AS slot_count)", SQL.identifier(self._x_rack_view()))
        )


class ParcelRack(models.Model, ParcelRackViewBuilder):
    _name = "parcel.rack"
    _description = "Parcel rack"
    _auto = False

    slot_count = fields.Integer()

    def _x_rack_view(self):
        return self._table
