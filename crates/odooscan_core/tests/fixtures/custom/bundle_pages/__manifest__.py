{
    "name": "Bundle pages",
    "version": "18.0.1.0.0",
    "depends": ["web"],
    "assets": {
        "web.assets_backend": [
            "bundle_pages/static/src/backend.js",
            "bundle_pages/static/src/both.js",
            "bundle_pages/static/src/stray.js",
        ],
        "web.assets_frontend": [
            ("include", "web.assets_frontend_minimal"),
            "bundle_pages/static/src/front.js",
            "bundle_pages/static/src/front_legacy.js",
            "bundle_pages/static/src/both.js",
        ],
        "web.assets_frontend_minimal": [
            "bundle_pages/static/src/minimal.js",
            "bundle_pages/static/src/early.js",
        ],
        "web.assets_frontend_lazy": [
            ("include", "web.assets_frontend"),
            "bundle_pages/static/src/late.js",
        ],
        "bundle_pages.lazy": ["bundle_pages/static/src/lazy.js"],
    },
    "license": "LGPL-3",
}
