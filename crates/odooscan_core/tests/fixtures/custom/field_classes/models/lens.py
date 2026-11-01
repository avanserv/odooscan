import odoo
from odoo import fields, models
from odoo import fields as f
from odoo.addons.field_classes.orm.vector import Link, Mark, Series, Subseries, Subvector, Tag, Vector
from odoo.addons.optics.orm import Thing


class Lens(models.Model):
    _name = "x.lens"
    _description = "Lens"

    embedding = Vector(size=3)
    coarse = Subvector()
    label = Tag()
    link = Link("res.partner")
    blob = fields.Serialized()
    note = odoo.fields.Char()
    code = f.Char()
    opaque = Thing()
    dated = Tag(default=fields.Date.today())
    series = Series()
    subseries = Subseries()
    mark = Mark("res.partner")
    other_mark = Mark("res.partner")
    mark_id = fields.Integer(related="mark")

    _embedding_index = models.Index("(embedding)")

    def _known_fields(self):
        self.env.cr.execute("SELECT lens.embedding FROM x_lens lens")
        return self.search(
            [
                ("embedding", "=", False),
                ("coarse", "=", False),
                ("label", "=", "a"),
                ("link", "=", 1),
                ("link.name", "=", "a"),
                ("blob", "=", False),
                ("note", "=", "a"),
                ("code", "=", "a"),
                ("series", "=", False),
                ("subseries", "=", False),
                ("mark", "=", 1),
                ("mark.name", "=", "a"),
            ]
        )

    def _unknown_fields(self):
        self.search([("embeding", "=", False)])
        self.search([("link.missing", "=", "a")])
        return self.search([("opaque", "=", False)])
