from odoo import fields, models


class ListedRoom(models.Model):
    _name = "unlisted.room.listed"
    _description = "Room with a row in a listed file"


class HookRoom(models.Model):
    _name = "unlisted.room.hook"
    _description = "Room with a row in a file the hook loads"


class UnlistedRoom(models.Model):
    _name = "unlisted.room.unlisted"
    _description = "Room with a row in a file nothing loads"


class TestsRoom(models.Model):
    _name = "unlisted.room.tests"
    _description = "Room with a row in a tests copy"


class XmlRoom(models.Model):
    _name = "unlisted.room.xml"
    _description = "Room with a record in a file nothing loads"


class LegacyRoom(models.Model):
    _name = "unlisted.room.legacy"
    _description = "Room with a row under a key Odoo ignores"


class Plate(models.Model):
    _name = "unlisted.plate"
    _description = "Plate"

    code = fields.Char(required=True)


class Badge(models.Model):
    _name = "unlisted.badge"
    _description = "Badge"
    _inherits = {"unlisted.plate": "plate_id"}

    plate_id = fields.Many2one("unlisted.plate", required=True, ondelete="cascade")


class Sign(models.Model):
    _name = "unlisted.sign"
    _description = "Sign"

    plate_id = fields.Many2one("unlisted.plate")
