# Odoo takes an instance of any subclass of Field as a field of the class that holds it: checked on
# every series from 9.0 to 20.0.
from odoo import fields


class Vector(fields.Field):
    type = "vector"
    column_type = ("vector", "vector")


class Tag(fields.Char):
    pass


class Link(fields.Many2one):
    pass


class Subvector(Vector):
    pass


# Field is generic from 18.0 to 20.0, so a subclass may subscript it.
class Series(fields.Field[list]):
    pass


class Subseries(Series[int]):
    pass


# field_classes_dep has a Many2one of this name, so the scan cannot tell which one a call means.
class Mark(fields.Char):
    pass
