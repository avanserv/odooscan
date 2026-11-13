{
    "name": "Modern",
    "version": "19.0.1.0.0",
    "depends": ["base"],
    "data": [
        "security/ir.model.access.csv",
        "data/tiers.xml",
        "data/calls.xml",
        "data/translated.xml",
        "data/res.partner-translated.csv",
        "views/cards.xml",
    ],
    "init_xml": ["data/modern_digest_view.sql", "data/tier_pages.xml"],
    "update_xml": ["data/modern_gauge_view.sql"],
    "assets": {"web.assets_backend": ["modern19/static/src/**/*"]},
    "license": "LGPL-3",
}
