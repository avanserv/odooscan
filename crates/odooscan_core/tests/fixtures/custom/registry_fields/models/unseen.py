from odoo import fields, models

# The scan does not resolve UNSEEN, so registry.shed has a parent it cannot see, which may give
# each name below that registry.shelter lacks.
UNSEEN = "registry.ledger"


class RegistryShelter(models.AbstractModel):
    _name = "registry.shelter"
    _description = "Shelter"

    name = fields.Char()


class RegistryShed(models.Model):
    _name = "registry.shed"
    _description = "Shed"
    _inherit = ["registry.shelter", UNSEEN]

    amount = fields.Monetary()
    level = fields.Float(compute="_compute_level")
    follower_ids = fields.Many2many("res.partner")
    watcher_ids = fields.Many2many("res.partner")
    label = fields.Char(string="Name")
    state = fields.Char(tracking=True)

    def _refresh(self):
        return super()._refresh()


class RegistryShedRack(models.Model):
    _name = "registry.shed.rack"
    _description = "Shed rack"

    company_id = fields.Many2one("res.company")
    shed_id = fields.Many2one("registry.shed", check_company=True)


class RegistryShedView(models.Model):
    _name = "registry.shed.view"
    _description = "Shed view"
    _inherit = ["registry.shelter", UNSEEN]
    _auto = False


# Only the class of registry.shed has the non-literal _inherit, which may give _restock.
class RegistryShedLoft(models.Model):
    _name = "registry.shed.loft"
    _description = "Shed loft"
    _inherit = "registry.shed"

    def _restock(self):
        return super()._restock()
