from odoo import http
from odoo.exceptions import AccessError
from odoo.http import request


class Picks(http.Controller):
    _MODELS = ("res.partner", "res.country")
    _FIELDS = ("name", "email")

    @http.route("/bad/picks/country", auth="user", type="json")
    def country(self, model, res_id):
        if model not in self._MODELS:
            raise AccessError("Not a model of the shop")
        return request.env[model].sudo().browse(int(res_id)).read(["display_name"])

    @http.route("/bad/picks/label", auth="user", type="json")
    def label(self, model, res_id):
        if model not in request.env:
            raise AccessError("No such model")
        records = request.env[model].sudo().browse(int(res_id))
        return records.read(["display_name"])

    @http.route("/bad/picks/run", auth="user", type="json")
    def run(self, partner_id, action):
        partner = request.env["res.partner"].browse(int(partner_id))
        getattr(partner, "action_%s" % action)()
        return getattr(partner, action)()

    @http.route("/bad/picks/field", auth="user", type="json")
    def field(self, partner_id, name, fields):
        partner = request.env["res.partner"].sudo().browse(int(partner_id))
        partner.read([f for f in fields if f in self._FIELDS])
        partner.read(fields)
        return partner[name]

    @http.route("/bad/picks/search", auth="user", type="json")
    def search(self, term, domain, groupby):
        partners = request.env["res.partner"].sudo()
        partners.search_count([("name", "ilike", term)])
        partners.read_group([], ["id:count"], groupby)
        return partners.search_read(domain, ["name"])
