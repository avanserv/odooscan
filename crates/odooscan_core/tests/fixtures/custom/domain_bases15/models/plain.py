from odoo import models


class Valued:
    sizes = {"small": 1, "large": 2}


class Hooked:
    def __init_subclass__(cls, **kwargs):
        super().__init_subclass__(**kwargs)


class PlainValued(models.Model, Valued):
    _name = "plain.valued"
    _description = "Python base with a dict, which holds no field from saas~14.4"


class PlainHooked(models.Model, Hooked):
    _name = "plain.hooked"
    _description = "Python base with a hook, which adds no field from saas~14.4"
