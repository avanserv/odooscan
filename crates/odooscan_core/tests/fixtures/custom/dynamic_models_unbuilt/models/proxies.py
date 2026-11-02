from odoo import fields, models

MODEL_NAME = "unbuilt.proxy"


class Helper:
    _name = None


class NoName(models.AbstractModel):
    _name = False


class ProxyFirst(models.AbstractModel):
    _register = False
    _name = MODEL_NAME


class ProxyLast(models.AbstractModel):
    _name = f"{MODEL_NAME}.last"
    _register = False


class UnbuiltHolder(models.Model):
    _name = "unbuilt.holder"

    large_id = fields.Many2one("bad.generated.large")

    class NestedHelper:
        _name = MODEL_NAME


class BareHelper:
    _name = MODEL_NAME


class ObjectHelper(object):
    _name = MODEL_NAME
