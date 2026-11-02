{
    "name": "Legacy 14",
    "version": "14.0.1.0.0",
    "depends": ["base"],
    "init_xml": ["views/ledger_menus.xml"],
    "update_xml": ["data/legacy_digest_view.sql", "views/ledger_views.xml"],
    "data": ["security/ir.model.access.csv", "views/ledger_reports.xml", "views/ledger_updates.xml", "security/rules.xml", "security/second_writes.xml"],
}
