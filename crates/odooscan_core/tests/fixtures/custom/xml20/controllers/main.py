from lxml import etree

from odoo import http


class Feed(http.Controller):
    @http.route("/xml20/feed", auth="user", methods=["POST"])
    def feed(self, feed):
        etree.fromstring(feed.encode())
        resolving = etree.XMLParser(resolve_entities=True)
        return etree.fromstring(feed.encode(), resolving)
