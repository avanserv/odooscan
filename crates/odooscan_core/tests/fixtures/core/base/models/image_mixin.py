from odoo import fields, models


class ImageMixin(models.AbstractModel):
    _name = "image.mixin"

    image_1920 = fields.Image()
