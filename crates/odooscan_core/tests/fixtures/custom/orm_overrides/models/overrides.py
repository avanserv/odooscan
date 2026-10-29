from odoo import api, models


class ResPartner(models.Model):
    _inherit = "res.partner"

    def write(self, vals):
        if not vals:
            return
        if "handle" in vals:
            vals["handle"] = vals["handle"].strip()
        super().write(vals)

    def copy(self, default=None):
        if not self:
            return None

        def with_suffix(name):
            return f"{name} (bis)"

        default = dict(default or {}, handle=with_suffix(self.handle))
        super().copy(default)

    @api.model
    def create(self, vals):
        vals.setdefault("handle", vals.get("name"))
        return super().create(vals)

    def unlink(self):
        self.mapped("child_ids").write({"active": False})
        super().unlink()

    @api.model
    def _guess_handle(self, vals):
        return vals.get("name", "").split(" ")[0]


class ResPartnerBatch(models.Model):
    _inherit = "res.partner"

    @api.model_create_multi
    def create(self, vals_list):
        for vals in vals_list:
            vals.setdefault("handle", vals.get("name"))
        return super().create(vals_list)

    def write(self, vals):
        if not vals:
            return
        result = super().write(vals)
        return result

    def copy(self, default=None):
        if self.env.context.get("no_copy"):
            return None
        return super().copy(default)
