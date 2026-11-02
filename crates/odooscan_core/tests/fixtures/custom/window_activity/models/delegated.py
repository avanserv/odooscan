from odoo import fields, models

PARENTS = {"res.partner": "partner_id"}
PARTNER = "res.partner"
DELEGATE = True
OPTIONS = {"delegate": True, "required": True, "ondelete": "cascade"}


class WindowDelegated(models.Model):
    _name = "window.delegated"
    _description = "Window delegated"
    _inherits = {"window.mailed": "mailed_id"}

    mailed_id = fields.Many2one("window.mailed", required=True, ondelete="cascade")


class WindowDelegatedBare(models.Model):
    _name = "window.delegated.bare"
    _description = "Window delegated bare"
    _inherits = {"window.noted": "noted_id"}

    noted_id = fields.Many2one("window.noted", required=True, ondelete="cascade")


class WindowUnreadValue(models.Model):
    _name = "window.unread.value"
    _description = "Window unread value"
    _inherits = PARENTS

    partner_id = fields.Many2one("res.partner", required=True, ondelete="cascade")


class WindowUnreadKey(models.Model):
    _name = "window.unread.key"
    _description = "Window unread key"
    _inherits = {PARTNER: "partner_id"}

    partner_id = fields.Many2one("res.partner", required=True, ondelete="cascade")


class WindowUnreadComodel(models.Model):
    _name = "window.unread.comodel"
    _description = "Window unread comodel"

    partner_id = fields.Many2one(PARTNER, delegate=True, required=True, ondelete="cascade")


class WindowUnreadFlag(models.Model):
    _name = "window.unread.flag"
    _description = "Window unread flag"

    partner_id = fields.Many2one("res.partner", delegate=DELEGATE, required=True, ondelete="cascade")


class WindowUnreadOptions(models.Model):
    _name = "window.unread.options"
    _description = "Window unread options"

    partner_id = fields.Many2one("res.partner", **OPTIONS)


class WindowUnreadAbsent(models.Model):
    _name = "window.unread.absent"
    _description = "Window unread absent"
    _inherits = {"window.absent": "absent_id"}

    absent_id = fields.Many2one("window.absent", required=True, ondelete="cascade")


class WindowUnreadPart(models.Model):
    _name = "window.unread.part"
    _description = "Window unread part"
    _inherit = ["window.absent"]


class WindowUnreadWhole(models.Model):
    _name = "window.unread.whole"
    _description = "Window unread whole"
    _inherits = {"window.unread.part": "part_id"}

    part_id = fields.Many2one("window.unread.part", required=True, ondelete="cascade")
