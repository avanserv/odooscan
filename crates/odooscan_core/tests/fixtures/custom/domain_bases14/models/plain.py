from odoo import fields, models


class Valued:
    sizes = {"small": 1, "large": 2}


class Hooked:
    def __init_subclass__(cls, **kwargs):
        super().__init_subclass__(**kwargs)


class Holder:
    label = fields.Char()


# Up to saas~14.3, setup takes every field it finds through the MRO, so any of these bases may
# give the model a field the scan cannot see.
class OldValued(models.Model, Valued):
    _name = "old.valued"
    _description = "Python base with a dict, which may hold a field"


class OldHooked(models.Model, Hooked):
    _name = "old.hooked"
    _description = "Python base with a hook, which may add a field"


class OldHeld(models.Model, Holder):
    _name = "old.held"
    _description = "Python base with a field"
