from odoo import api, fields, models


class DriftProbe(models.Model):  # odooscan: ignore[OD401]
    _name = "drift.probe"
    _description = "Drift probe"
    _log_access = False

    name = fields.Char()


class DriftProbeLogged(models.Model):
    _inherit = "drift.probe"
    _log_access = True


class DriftGauge(models.Model):  # odooscan: ignore[OD401]
    _name = "drift.gauge"
    _description = "Drift gauge"

    name = fields.Char()


class DriftGaugeQuiet(models.Model):
    _inherit = "drift.gauge"
    _log_access = False


class DriftSensor(models.Model):  # odooscan: ignore[OD401]
    _name = "drift.sensor"
    _description = "Drift sensor"
    _log_access = False

    name = fields.Char()

    def x_drift_stamps(self):
        self.env.cr.execute("SELECT p.create_date FROM drift_probe p")
        self.env.cr.execute("SELECT g.write_date FROM drift_gauge g")
        self.env.cr.execute("SELECT s.create_uid FROM drift_sensor s")
        return self.env.cr.fetchall()


class DriftProbeMonogram(models.Model):
    _inherit = "drift.probe"

    monogram = fields.Char(compute="_compute_monogram")

    @api.depends("name")
    def _compute_monogram(self):
        for probe in self:
            if probe.name:
                probe.monogram = probe.name[0]
