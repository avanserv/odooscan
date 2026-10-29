from odoo import fields, models, tools


class ParcelDepot(models.Model):
    _name = "parcel.depot"
    _description = "Parcel depot"
    _parent_store = True  # odooscan: ignore[OD217]

    name = fields.Char()
    parent_id = fields.Many2one("parcel.depot", ondelete="cascade")


class ParcelBox(models.Model):
    _name = "parcel.box"
    _description = "Parcel box"

    name = fields.Char()
    depot_id = fields.Many2one("parcel.depot")
    weight_uom_id = fields.Many2one("res.currency")
    total_weight = fields.Float(compute="_compute_total_weight")
    item_ids = fields.One2many("parcel.item", "box_id")
    tag_ids = fields.Many2many("res.country")
    label_image = fields.Binary()
    handling_fee = fields.Float(company_dependent=True)
    handling_note = fields.Char(company_dependent=True, store=False)
    preview_image = fields.Image(compute="_compute_preview_image")
    pickup_code = fields.Char(compute="_compute_pickup_code", inverse="_inverse_pickup_code")
    seal_code = fields.Char(compute=None)
    seal_ref = fields.Char(related=False)

    def _compute_pickup_code(self):
        for box in self:
            box.pickup_code = box.name

    def _inverse_pickup_code(self):
        for box in self:
            box.name = box.pickup_code

    def _compute_total_weight(self):
        for box in self:
            box.total_weight = sum(box.item_ids.mapped("weight"))

    def _compute_preview_image(self):
        for box in self:
            box.preview_image = box.label_image


class ParcelItem(models.Model):
    _name = "parcel.item"
    _description = "Parcel item"

    box_id = fields.Many2one("parcel.box")
    weight = fields.Float()


class ParcelLocker(models.Model):
    _name = "parcel.locker"
    _description = "Parcel locker"
    _inherits = {"parcel.depot": "depot_id"}

    depot_id = fields.Many2one("parcel.depot", required=True, ondelete="cascade")
    code = fields.Char()


class ParcelKiosk(models.Model):
    _name = "parcel.kiosk"
    _description = "Parcel kiosk"
    _inherits = {"parcel.depot": "host_depot_id"}

    screen = fields.Char()


class ParcelReport(models.Model):
    _name = "parcel.report"
    _description = "Parcel report"
    _auto = False

    box_count = fields.Integer()


class ParcelStat(models.Model):
    _name = "parcel.stat"
    _description = "Parcel statistic"
    _table_query = "SELECT 1 AS id, 0 AS box_count"

    box_count = fields.Integer()


class ParcelDigest(models.Model):
    _name = "parcel.digest"
    _description = "Parcel digest"

    box_count = fields.Integer()

    @property
    def _table_query(self):
        return "SELECT 1 AS id, 0 AS box_count"


class ParcelSlot(models.Model):
    _name = "parcel.slot"
    _description = "Parcel slot"
    _table = "parcel_slot_a"

    name = fields.Char()


class ParcelSlotMoved(models.Model):
    _inherit = "parcel.slot"
    _table = "parcel_slot_b"


class ParcelRoute(models.Model):
    _name = "parcel.route"
    _description = "Parcel route"
    _table = "parcel_leg"

    name = fields.Char()


class ParcelLeg(models.Model):
    _name = "parcel.leg"
    _description = "Parcel leg"

    name = fields.Char()


class ParcelScanner(models.Model):
    _name = "parcel.scanner"
    _description = "Parcel scanner"

    serial = fields.Char(compute="_compute_serial")
    firmware = fields.Char(compute="_compute_serial")

    def init(self):
        self.env.cr.execute("ALTER TABLE parcel_scanner ADD COLUMN IF NOT EXISTS serial varchar")
        tools.create_column(self.env.cr, "parcel_scanner", "firmware", "varchar")

    def _compute_serial(self):
        self.env.cr.execute(
            "SELECT s.id, s.serial, s.firmware FROM parcel_scanner s WHERE s.id IN %s", [tuple(self.ids)]
        )
        rows = {row[0]: row[1:] for row in self.env.cr.fetchall()}
        for scanner in self:
            scanner.serial, scanner.firmware = rows.get(scanner.id, (False, False))


class ParcelShelf(models.Model):
    _name = "parcel.shelf"
    _description = "Parcel shelf"

    code = fields.Char()
    occupancy = fields.Float(compute="_compute_occupancy")

    def init(self):
        self.env.cr.execute("CREATE INDEX IF NOT EXISTS parcel_shelf_code_index ON parcel_shelf (code)")
        self.env.cr.execute("ALTER TABLE parcel_box ADD COLUMN IF NOT EXISTS occupancy numeric")

    def _compute_occupancy(self):
        for shelf in self:
            shelf.occupancy = len(shelf.code or "")


class ParcelBeacon(models.Model):
    _name = "parcel.beacon"
    _description = "Parcel beacon"
    _log_access = False
    _sql_constraints = [("signal_stamp", "CHECK(signal > 0 OR write_date IS NULL)", "A beacon signal is positive.")]

    signal = fields.Integer()


class ParcelTrackingMixin:
    def _x_tracking_label(self):
        return "tracked"


class ParcelCrate(models.Model, ParcelTrackingMixin):
    _name = "parcel.crate"
    _description = "Parcel crate"

    name = fields.Char()
