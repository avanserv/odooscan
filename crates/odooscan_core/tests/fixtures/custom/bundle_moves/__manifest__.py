{
    "name": "Bundle moves",
    "version": "18.0.1.0.0",
    "depends": ["web"],
    "assets": {
        "web.assets_backend": [
            "bundle_moves/static/src/*.js",
            "bundle_moves/static/src/dropped/*",
            "bundle_moves/static/src/panels/old_panel.js",
            (
                "replace",
                "bundle_moves/static/src/panels/old_panel.js",
                "bundle_moves/static/src/panels/new_panel.js",
            ),
            ("include", "bundle_moves._shared"),
            ("remove", "bundle_moves/static/src/frontend.js"),
            ("remove", "bundle_moves/static/src/dropped/*"),
        ],
        "web.assets_frontend": ["bundle_moves/static/src/frontend.js"],
        "bundle_moves._shared": [
            "bundle_moves/static/src/shared/shared.js",
            ("remove", "bundle_moves/static/src/hidden.js"),
        ],
    },
    "license": "LGPL-3",
}
