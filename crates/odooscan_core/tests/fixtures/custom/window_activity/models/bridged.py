from odoo import fields, models


class WindowBridgedPlain(models.Model):
    _name = "window.bridged.plain"
    _description = "Window bridged plain"

    name = fields.Char()


class WindowBridgedListed(models.Model):
    _name = "window.bridged.listed"
    _description = "Window bridged listed"

    name = fields.Char()


class WindowBridgedMixin(models.Model):
    _name = "window.bridged.mixin"
    _description = "Window bridged mixin"

    name = fields.Char()


class WindowBridgedDelegate(models.Model):
    _name = "window.bridged.delegate"
    _description = "Window bridged delegate"

    name = fields.Char()


class WindowBridgedRead(models.Model):
    _name = "window.bridged.read"
    _description = "Window bridged read"

    name = fields.Char()
