{
    "name": "Data fields",
    "version": "20.0.1.0.0",
    "depends": ["base"],
    "data": [
        "security/ir.model.access.csv",
        "views/partner_views.xml",
        "views/ticket_views.xml",
        "data/ir.model.fields.csv",
        "data/fields.xml",
        "data/partners.xml",
        "security/rules.xml",
    ],
    "demo": ["demo/views.xml", "demo/fields.xml"],
    "pre_init_hook": "_x_pre_init",
    "post_init_hook": "_x_post_init",
    "license": "LGPL-3",
}
