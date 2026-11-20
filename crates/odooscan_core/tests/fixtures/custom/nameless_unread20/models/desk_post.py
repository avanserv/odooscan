try: from odoo.models import Model
except ImportError: Model = object


class DeskPost(Model):
    _description = "Desk post"


for _ in range(1):
    from odoo import models


class DeskPin(models.Model):
    _description = "Desk pin, on a base imported in a loop"


print "loaded"
