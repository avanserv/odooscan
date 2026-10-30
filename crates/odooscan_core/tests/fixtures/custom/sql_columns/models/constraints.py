from odoo import models


class ParcelBoxRules(models.Model):
    _inherit = "parcel.box"
    _sql_constraints = [
        ("depot_required", "CHECK(depot IS NOT NULL)", "A box needs a depot."),
        ("weight_positive", "CHECK(total_weight >= 0)", "A box cannot weigh less than nothing."),
        ("tag_unique", "UNIQUE(name, tag_ids)", "Tags are unique per box name."),
        ("title_present", "CHECK(display_name IS NOT NULL)", "A box needs a title."),
        (
            "name_shape",
            "CHECK (char_length(name) > 0 AND weight_uom_id::text <> '' AND name NOT ILIKE 'tmp%' "
            "AND create_date > date '2001-01-01' AND CAST(handling_fee AS integer) >= 0)",
            "A box name is not a draft.",
        ),
        ("depot_link", "FOREIGN KEY (depot_id) REFERENCES parcel_hub (hub_ref) ON DELETE CASCADE", "Depot link."),
        ("name_trigram", "EXCLUDE USING gist (name gist_trgm_ops WITH =)", "Names differ."),
        ("code_limit", "CHECK(code_limit > %s)" % 10, "Codes stay small."),
    ]
    _name_index = models.Index("(bogus_name)")
    _code_check = models.Constraint("CHECK(bogus_code <> '')", "Odoo reads no constraint object before 19.0.")


class ParcelLockerRules(models.Model):
    _inherit = "parcel.locker"
    _sql_constraints = [("code_per_depot", "UNIQUE(name, code)", "A locker code is unique per depot.")]


class ParcelRulesMixin(models.AbstractModel):
    _name = "parcel.rules.mixin"
    _description = "Parcel rules mixin"
    _sql_constraints = [("bogus_positive", "CHECK(bogus > 0)", "Odoo applies no constraint to a mixin.")]


class ResPartnerRules(models.Model):
    _inherit = "res.partner"
    _sql_constraints = [("name_type_unique", "UNIQUE(name, type, short_name)", "A contact name is unique per type.")]
