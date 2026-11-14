{
    "name": "Bundle pages partial",
    "version": "18.0.1.0.0",
    "depends": ["web", "ghost_enterprise"],
    "assets": {
        "web.assets_backend": ["bundle_pages_partial/static/src/backend.js"],
        "web.assets_frontend": ["bundle_pages_partial/static/src/front.js"],
    },
    "license": "LGPL-3",
}
