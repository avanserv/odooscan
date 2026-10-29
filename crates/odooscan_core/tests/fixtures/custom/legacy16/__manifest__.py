{
    "name": "Legacy",
    "version": "16.0.1.0.0",
    "depends": ["base", "web"],
    "data": ["security/ir.model.access.csv", "views/views.xml"],
    "update_xml": ["data/legacy_gauge_view.sql"],
    "assets": {"web.assets_backend": ["legacy16/static/src/**/*"]},
}
