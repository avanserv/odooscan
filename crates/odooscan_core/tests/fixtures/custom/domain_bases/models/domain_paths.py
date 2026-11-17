from odoo import api, fields, models


class DomainHiddenPaths(models.Model):
    _inherit = "domain.hidden"

    past_hidden = fields.Char(related="whole_id.no_past_hidden")

    @api.onchange("stamp")
    def _onchange_stamp(self):
        self.past_hidden = False


class DomainComputedPaths(models.Model):
    _inherit = "domain.computed"
    _order = "stamp"


class DomainUnreadInheritsPaths(models.Model):
    _inherit = "domain.unread.inherits"

    stamp_size = fields.Integer(compute="_compute_stamp_size")

    @api.depends("stamp")
    def _compute_stamp_size(self):
        for record in self:
            record.stamp_size = len(record.stamp or "")


class DomainWholePaths(models.Model):
    _inherit = "domain.whole"
    _order = "no_whole_order"

    hidden_stamp = fields.Char(related="hidden_id.stamp")
    parent_stamp = fields.Char(related="parent_id.hidden_id.stamp")


class DomainChainedPaths(models.Model):
    _inherit = "domain.chained"

    @api.onchange("no_chained_onchange")
    def _onchange_no_chained(self):
        self.stamp = False


class DomainFoundPaths(models.Model):
    _inherit = "domain.found"
    _order = "side_stamp"
    _rec_names_search = ["label.no_past_search"]

    side_label = fields.Char(related="side_stamp")
    label_size = fields.Integer(compute="_compute_label_size")

    @api.depends("label.no_past_depends")
    def _compute_label_size(self):
        for record in self:
            record.label_size = len(record.label or "")
