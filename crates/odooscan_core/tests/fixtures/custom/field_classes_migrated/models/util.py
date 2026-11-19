# The upgrade script imports the field class of this name from field_classes, but Odoo loads it by
# its path outside the module, so a model of this module holds a plain object.
class Vector:
    pass
