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


# orm/mark.py has a Many2one of this name, so the scan cannot tell which one a call means.
class Mark(fields.Char):
    pass


# Odoo builds a field from the keywords its __init__ chain hands to Field.__init__, so a class with
# its own __init__ may read any argument as any attribute: here the first one is the label.
class CompanyLink(fields.Many2one):
    def __init__(self, string=None, **kwargs):
        super().__init__("res.company", string=string, **kwargs)


class SubCompanyLink(CompanyLink):
    pass


# orm/mark.py has a Char of this name, so the scan cannot tell whether a call means a Many2many.
class Pair(fields.Many2many):
    pass
