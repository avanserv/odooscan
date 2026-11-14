{
    "name": "Legacy",
    "version": "16.0.1.0.0",
    "depends": ["base", "web"],
    "data": ["security/ir.model.access.csv", "views/views.xml", "views/assets.xml"],
    "update_xml": ["data/legacy_gauge_view.sql"],
    "qweb": ["static/src/xml/widget.xml"],
    "assets": {
        "web.assets_backend": ["legacy16/static/src/js/**/*"],
        "web.assets_qweb": ["legacy16/static/src/xml/widget.xml"],
    },
    "license": "LGPL-3",
}
