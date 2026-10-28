# Changelog

## [0.4.0](https://github.com/avanserv/odooscan/compare/v0.3.0...v0.4.0) (2026-10-28)


### Features

* **checks:** Check the field names in literal domains ([#23](https://github.com/avanserv/odooscan/issues/23)) ([e782838](https://github.com/avanserv/odooscan/commit/e782838543c24a246f07a77594a2254b1f3199be))
* **checks:** Report a super() call no parent class can answer ([#24](https://github.com/avanserv/odooscan/issues/24)) ([6535b23](https://github.com/avanserv/odooscan/commit/6535b234ef042672db66f8e04a20d86036717c96))
* **checks:** Report definitions Odoo refuses when it loads the registry ([#22](https://github.com/avanserv/odooscan/issues/22)) ([9cbc530](https://github.com/avanserv/odooscan/commit/9cbc530e138cf193a1723c36a256c91b842e6852))


### Bug Fixes

* **checks:** Silence the false positives Odoo core raises ([#21](https://github.com/avanserv/odooscan/issues/21)) ([67f648d](https://github.com/avanserv/odooscan/commit/67f648dcd11933aba12bc57ff46be525f4e697bb))
* **project:** Print the series notice only when no series applies ([#20](https://github.com/avanserv/odooscan/issues/20)) ([ae5dc7a](https://github.com/avanserv/odooscan/commit/ae5dc7a93cbc5b0a0aedc9d2149ddcae6f2fec2c))

## [0.3.0](https://github.com/avanserv/odooscan/compare/v0.2.0...v0.3.0) (2026-10-27)


### Features

* **checks:** Report inherited view locators that match nothing ([#17](https://github.com/avanserv/odooscan/issues/17)) ([6d6edca](https://github.com/avanserv/odooscan/commit/6d6edca622cd5f28299456dce684faa60aa6e9c2))
* **python:** Check the XML ids Python code references ([#16](https://github.com/avanserv/odooscan/issues/16)) ([bd5f760](https://github.com/avanserv/odooscan/commit/bd5f760815aac8c5bf484659c9a88918f7d64891))

## [0.2.0](https://github.com/avanserv/odooscan/compare/v0.1.0...v0.2.0) (2026-10-27)


### ⚠ BREAKING CHANGES

* Install the scanner as oscn ([#6](https://github.com/avanserv/odooscan/issues/6))

### Features

* **cli:** Hide known findings with a baseline file ([#8](https://github.com/avanserv/odooscan/issues/8)) ([d82d454](https://github.com/avanserv/odooscan/commit/d82d4542828aaec193fd1cf22b7ab5859ba3383c))
* **cli:** Read settings from odooscan.toml or pyproject.toml ([#7](https://github.com/avanserv/odooscan/issues/7)) ([f86b1ef](https://github.com/avanserv/odooscan/commit/f86b1efee05e3bfbb35220c739a99443b3252bfe))
* **cli:** Report as GitHub annotations and GitLab Code Quality ([#9](https://github.com/avanserv/odooscan/issues/9)) ([719ccdc](https://github.com/avanserv/odooscan/commit/719ccdc7103a63824c1046faf1d923099fd8d196))
* Install the scanner as oscn ([#6](https://github.com/avanserv/odooscan/issues/6)) ([c04564c](https://github.com/avanserv/odooscan/commit/c04564c4408ab5ccc92dd28cf8b6cbeaaaba4f98))

## 0.1.0 (2026-10-27)


### Features

* Check Odoo modules against an index of every module they build on ([104d89a](https://github.com/avanserv/odooscan/commit/104d89a2c5e184c5d20ead765941dcf43ebd1e6a))
