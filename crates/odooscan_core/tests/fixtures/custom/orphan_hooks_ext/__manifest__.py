{
    "name": "Orphan hooks extension",
    "version": "18.0.1.0.0",
    "depends": ["orphan_hooks"],
    "data": [
        "security/ir.model.access.csv",
        "data/cron.xml",
        "views/orphan_hook.xml",
    ],
    "assets": {"web.assets_backend": ["orphan_hooks_ext/static/src/**/*"]},
    "license": "LGPL-3",
}
