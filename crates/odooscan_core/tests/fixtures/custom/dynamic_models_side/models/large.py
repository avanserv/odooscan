from odoo import fields, models


class SideLarge(models.Model):
    _name = "bad.generated.large"

    name = fields.Char()


class SideHuge(models.Model):
    _name = "bad.generated.huge"

    name = fields.Char()


class SideOrder(models.Model):
    _name = "side.order"

    name = fields.Char()


class SideOrderLine(models.Model):
    _name = "side.order.line"

    name = fields.Char()


class SidelineNote(models.Model):
    _name = "sideline.note"

    name = fields.Char()
