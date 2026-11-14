{
    "name": "Asset paths matching no file",
    "version": "18.0.1.0.0",
    "depends": ["web"],
    "license": "LGPL-3",
    "assets": {
        "web.assets_backend": [
            "manifest_assets_unmatched/static/src/jss/**/*",
            "manifest_assets_unmatched/static/src/js/missing.js",
            ("remove", "manifest_assets_unmatched/static/src/legacy/**/*.js"),
            "manifest_assets_unmatched/views/*.js",
            "manifest_assets_unmatched/static/src/js/**/*",
            ("include", "web._assets_helpers"),
            "https://cdn.example.com/shelf.js",
            "web/static/src/shelves/**/*",
            "manifest_assets_unmatched/static/fonts/shelf.woff2",
        ],
    },
}
