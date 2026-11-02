{
    "name": "Bad sale",
    "version": "18.0.1.0.0",
    "license": "LGPL-3",
    "depends": ["base", "not_installed"],
    "data": [
        "security/ir.model.access.csv",
        "views/views.xml",
        "views/late.xml",
        "views/locators.xml",
        "data/missing.xml",
        "data/assigned.xml",
        "views/domains.xml",
    ],
    "assets": {"web.assets_backend": ["bad_sale/static/src/js/**/*"]},
}
