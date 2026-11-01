from odoo import models

KIND = "first"


class UnparsedBuilt(models.Model):
    _name = "unparsed.built.%s" % KIND

    def _close(self):
        print "closing"
