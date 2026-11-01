from openerp.osv import fields, osv


# 9.0 aliases each of these bases to an ORM model base, so none hides a parent.
class Legacy9Ledger(osv.osv):
    _name = "legacy9.ledger"

    _columns = {"name": fields.char("Name")}


class Legacy9Prompt(osv.osv_memory):
    _name = "legacy9.prompt"

    _columns = {"name": fields.char("Name")}


class Legacy9Mixin(osv.osv_abstract):
    _name = "legacy9.mixin"

    _columns = {"note": fields.text("Note")}


class Legacy9Gauge(osv.osv):
    _name = "legacy9.gauge"

    _columns = {"name": fields.char("Name")}
