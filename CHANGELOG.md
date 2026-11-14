# Changelog

## [0.12.0](https://github.com/avanserv/odooscan/compare/v0.11.0...v0.12.0) (2026-11-14)


### Features

* **checks:** Report addon imports outside depends or of missing names ([#303](https://github.com/avanserv/odooscan/issues/303)) ([383329b](https://github.com/avanserv/odooscan/commit/383329b5f189244edba27bdc2b2dba0b83c5200d))
* **checks:** Report dependencies a module takes nothing from ([#305](https://github.com/avanserv/odooscan/issues/305)) ([320cc2a](https://github.com/avanserv/odooscan/commit/320cc2ae71ad6481d39036e76d3da720dfe9d084))
* **checks:** Report dependencies another dependency already brings ([#304](https://github.com/avanserv/odooscan/issues/304)) ([fe17855](https://github.com/avanserv/odooscan/commit/fe17855fda861d23c3981ff0e82ff82cb5c569be))
* **checks:** Report migration folders Odoo skips and scripts it refuses ([#299](https://github.com/avanserv/odooscan/issues/299)) ([5755f00](https://github.com/avanserv/odooscan/commit/5755f00468087de1222c8db39daf58e6ecea78f5))
* **checks:** Report Python files and tests no __init__.py imports ([#301](https://github.com/avanserv/odooscan/issues/301)) ([1642c4d](https://github.com/avanserv/odooscan/commit/1642c4d2155ba31a53ef0a8ae34f4e07978ca618))
* **checks:** Report shadowed route paths and undecorated overrides ([#300](https://github.com/avanserv/odooscan/issues/300)) ([fe0a4e6](https://github.com/avanserv/odooscan/commit/fe0a4e64a14735d2dc939f12d3f2f8c974e15dc0))
* **manifest:** Check the license, version, depends, data and assets keys ([#298](https://github.com/avanserv/odooscan/issues/298)) ([6a8733f](https://github.com/avanserv/odooscan/commit/6a8733f0ece05c35489a5828bdedfb53978d86dc))


### Bug Fixes

* **checks:** Gate OD514 and OD515 by series and skip folders Odoo skips ([#315](https://github.com/avanserv/odooscan/issues/315)) ([36e8ee4](https://github.com/avanserv/odooscan/commit/36e8ee476d9d584df2a14fb70f9bf672bd35d845))
* **checks:** Read __all__ grown inside loops in OD516 ([#330](https://github.com/avanserv/odooscan/issues/330)) ([c89c926](https://github.com/avanserv/odooscan/commit/c89c9262b8a242394a3019c1b135887118c1af5e))
* **checks:** Read unread importers, grown __all__, extensions in OD516 ([#320](https://github.com/avanserv/odooscan/issues/320)) ([06b2406](https://github.com/avanserv/odooscan/commit/06b2406c6fe937e8899cdffd4d78cbf786e4f5ea))
* **checks:** Report OD231 once per clash, on served paths ([#318](https://github.com/avanserv/odooscan/issues/318)) ([19856fe](https://github.com/avanserv/odooscan/commit/19856fee4901ed28dcb8bc71f7471ad433f7a7b9))
* **checks:** Report XML assets outside static before saas~16.3 in OD513 ([#328](https://github.com/avanserv/odooscan/issues/328)) ([12f98c6](https://github.com/avanserv/odooscan/commit/12f98c626389809071d93933eff6d058392310d5))
* **checks:** Say in OD503 that a dependency's manifest cannot be read ([#326](https://github.com/avanserv/odooscan/issues/326)) ([313efca](https://github.com/avanserv/odooscan/commit/313efca8c666dc85f0c49a11e0eff5f00513ff5f))
* **checks:** Word OD518 like OD202, correct the OD518 and OD519 pages ([#323](https://github.com/avanserv/odooscan/issues/323)) ([fe998b2](https://github.com/avanserv/odooscan/commit/fe998b2a65bc6ddb6332c3443fdb4e70da00fe59))
* **csvdata:** Read nothing more from a CSV that OD000 refuses ([#310](https://github.com/avanserv/odooscan/issues/310)) ([ef1f105](https://github.com/avanserv/odooscan/commit/ef1f1052ea80780ddb67da1867c5a30b699519e3))
* **csvdata:** Report a CSV row or an undecodable file Odoo cannot load ([#297](https://github.com/avanserv/odooscan/issues/297)) ([1fca3a2](https://github.com/avanserv/odooscan/commit/1fca3a2cc8ae9d984815077210e5f4c139b2923b))
* **imports:** Read suppress, type-only defs and match captures ([#322](https://github.com/avanserv/odooscan/issues/322)) ([a9548ae](https://github.com/avanserv/odooscan/commit/a9548aebb96dea5e3ad4956dc01ed4a4302516fe))
* **index:** Keep OD521 silent on a rootless tree only for its model ([#333](https://github.com/avanserv/odooscan/issues/333)) ([e981707](https://github.com/avanserv/odooscan/commit/e9817079f93ca4a5ae2b14959859c0c83c565f0e))
* **index:** Keep OD521 silent on a view tree the scan cannot read ([#324](https://github.com/avanserv/odooscan/issues/324)) ([44bad7e](https://github.com/avanserv/odooscan/commit/44bad7e503d983bd70352f705218cbe47a4975a1))
* **index:** Keep OD521 silent on an unread branch below the root ([#332](https://github.com/avanserv/odooscan/issues/332)) ([47c310b](https://github.com/avanserv/odooscan/commit/47c310b220d1cda71260a2ba728fe80ff5d20e59))
* **index:** Rank a module with no depends after base from saas~18.2 ([#334](https://github.com/avanserv/odooscan/issues/334)) ([a39de31](https://github.com/avanserv/odooscan/commit/a39de31c210a7f4a82eae8e7266197e51c83b50a))
* **index:** Read an empty depends as base from saas~18.2 ([#327](https://github.com/avanserv/odooscan/issues/327)) ([ea4a05b](https://github.com/avanserv/odooscan/commit/ea4a05b13d94d4657c071b24f6ee821008fdb638))
* **manifest:** Read a manifest written as a list of key/value pairs ([#306](https://github.com/avanserv/odooscan/issues/306)) ([e4dcda6](https://github.com/avanserv/odooscan/commit/e4dcda68b7579b7d87c7f9caecc29f40f5c7cea4))
* **manifest:** Refuse an unhashable item of a nested set ([#325](https://github.com/avanserv/odooscan/issues/325)) ([fc5d0a9](https://github.com/avanserv/odooscan/commit/fc5d0a9d4f9c619d2c0bfbce46a59150d8f0babe))
* **manifest:** Report a saas version behind the plain series in OD509 ([#336](https://github.com/avanserv/odooscan/issues/336)) ([c93ed8c](https://github.com/avanserv/odooscan/commit/c93ed8c82d980980e27794dc0a6d1bedb7a1ea9c))
* **manifest:** Report OD511 on an empty depends and word OD507 by series ([#312](https://github.com/avanserv/odooscan/issues/312)) ([e80b78a](https://github.com/avanserv/odooscan/commit/e80b78a39d6f62aef6bbcc00ea373673911476d5))
* **manifest:** Word manifest notices and judge Python 2 strings ([#308](https://github.com/avanserv/odooscan/issues/308)) ([08d8eae](https://github.com/avanserv/odooscan/commit/08d8eae07a121a23c417d4c338196560ecb693c4))
* **project:** Report a checked manifest that does not parse ([#296](https://github.com/avanserv/odooscan/issues/296)) ([688f6e5](https://github.com/avanserv/odooscan/commit/688f6e509f592aa2790170db03394bfbc39708d2))
* **python:** Leave loop-bound migrate unjudged, read stage scripts only ([#316](https://github.com/avanserv/odooscan/issues/316)) ([20dfca4](https://github.com/avanserv/odooscan/commit/20dfca4a9a1b7281ebdd7627222011a42e4c268b))


### Performance Improvements

* **checks:** Find OD516's unread dependents once per index ([#331](https://github.com/avanserv/odooscan/issues/331)) ([9ea6d53](https://github.com/avanserv/odooscan/commit/9ea6d53a0954789e48fa5e3a76017cad13f29796))

## [0.11.0](https://github.com/avanserv/odooscan/compare/v0.10.0...v0.11.0) (2026-11-13)


### Features

* **checks:** Report unsanitized Html, secrets and open portal rules ([#251](https://github.com/avanserv/odooscan/issues/251)) ([c808fed](https://github.com/avanserv/odooscan/commit/c808fed033c29ad8a85c54f42d0ec1b67d475501))
* **python:** Report GET-route writes, open CORS and userless auth=none ([#249](https://github.com/avanserv/odooscan/issues/249)) ([aac10b1](https://github.com/avanserv/odooscan/commit/aac10b1f48305ef2c8364371343d04289ffc6ee4))
* **python:** Report superuser routes, session switches, secret == ([#250](https://github.com/avanserv/odooscan/issues/250)) ([b4bf247](https://github.com/avanserv/odooscan/commit/b4bf247de7f8d71e76a4cb6c84c71d9a4fe2c443))
* **taint:** Add sources, sinks and sanitizers from odooscan.toml ([#242](https://github.com/avanserv/odooscan/issues/242)) ([3c74f01](https://github.com/avanserv/odooscan/commit/3c74f0105fed1a781af364be2d11df9e6f55e2a9))
* **taint:** Follow sudo callers, deeper returns and self attributes ([#243](https://github.com/avanserv/odooscan/issues/243)) ([6bd4f2a](https://github.com/avanserv/odooscan/commit/6bd4f2ab3ddfc1bf44da2def954ab1a64d0f9cee))
* **taint:** Report a public attachment created as a user granted create ([#291](https://github.com/avanserv/odooscan/issues/291)) ([3fa7c52](https://github.com/avanserv/odooscan/commit/3fa7c52b440959e9fba314ced59ba16be87fda44))
* **taint:** Report command, deserialization, XML entity and header flows ([#245](https://github.com/avanserv/odooscan/issues/245)) ([5f9bc13](https://github.com/avanserv/odooscan/commit/5f9bc130d745d5a8a51166ded3c2e91d957cc6f7))
* **taint:** Report records a route returns by a user's id under sudo() ([#247](https://github.com/avanserv/odooscan/issues/247)) ([c300601](https://github.com/avanserv/odooscan/commit/c300601caec9f0d44d4d6ed3d76950bf6a51d3ac))
* **taint:** Report user content stored as a public attachment under sudo() ([#248](https://github.com/avanserv/odooscan/issues/248)) ([a0c9dd4](https://github.com/avanserv/odooscan/commit/a0c9dd45a919dfb182fbd6a209c022e899b8d56d))
* **taint:** Report user-picked models, methods, fields and domains ([#246](https://github.com/avanserv/odooscan/issues/246)) ([5d214f5](https://github.com/avanserv/odooscan/commit/5d214f52f769eb8b0e7ff106c99363a12e9aca30))


### Bug Fixes

* **checks:** Count base.group_everyone as public in OD402 ([#275](https://github.com/avanserv/odooscan/issues/275)) ([2a1fc96](https://github.com/avanserv/odooscan/commit/2a1fc96b7d9c92b63d363602caa2a8162df8b652))
* **checks:** Read OD405 and OD406 rules by public groups and booleans ([#276](https://github.com/avanserv/odooscan/issues/276)) ([95cb1e1](https://github.com/avanserv/odooscan/commit/95cb1e1a1efe73b7892d7985871a74872e304e67))
* **flows:** Follow a public method's parameter from a Python caller ([#258](https://github.com/avanserv/odooscan/issues/258)) ([0fae733](https://github.com/avanserv/odooscan/commit/0fae73368439ef7d4ad8735fa0fee5bc82119578))
* **flows:** Key the solver's memos by depth ([#254](https://github.com/avanserv/odooscan/issues/254)) ([a622925](https://github.com/avanserv/odooscan/commit/a6229257481941a1b7bdb5594f92aefe2e0672a7))
* **python:** Narrow OD131 change checks and skip OD130 uid restores ([#274](https://github.com/avanserv/odooscan/issues/274)) ([af0331f](https://github.com/avanserv/odooscan/commit/af0331ff94f78332cb2f35501c0f350a38692aac))
* **python:** Read None users and env locals in OD127 and OD129 ([#273](https://github.com/avanserv/odooscan/issues/273)) ([68fc1f5](https://github.com/avanserv/odooscan/commit/68fc1f5c56fd3cbb38a5e94661ac816f851171d4))
* **python:** Read route params per URL and inherited routing ([#261](https://github.com/avanserv/odooscan/issues/261)) ([e1c8594](https://github.com/avanserv/odooscan/commit/e1c8594c7dcbcc21eb09f94903a7a72f1c2aed72))
* **routing:** Find a route's class by position, bind bases by name ([#284](https://github.com/avanserv/odooscan/issues/284)) ([c706d05](https://github.com/avanserv/odooscan/commit/c706d050892eafe289598b9159b262403efceeb0))
* **taint:** Bind a for loop's target again before each pass of its body ([#259](https://github.com/avanserv/odooscan/issues/259)) ([e4cc1b2](https://github.com/avanserv/odooscan/commit/e4cc1b27d86e7152a33d56b52d961dfbe7b798ee))
* **taint:** Carry a continue arm's writes to the next loop pass ([#283](https://github.com/avanserv/odooscan/issues/283)) ([bed4ce3](https://github.com/avanserv/odooscan/commit/bed4ce3d177493fbaec7595cbc38004b85f3f7aa))
* **taint:** Confine a value inside an if x in allowed branch ([#266](https://github.com/avanserv/odooscan/issues/266)) ([c55daec](https://github.com/avanserv/odooscan/commit/c55daec256efa9e485a6a5748f0a5da99117e9ae))
* **taint:** Follow a value set on an attribute into its reads ([#281](https://github.com/avanserv/odooscan/issues/281)) ([e1fde24](https://github.com/avanserv/odooscan/commit/e1fde24445bd324af2a304d83a338b71b38b4e7b))
* **taint:** Keep every origin of a joined value in the summary edges ([#257](https://github.com/avanserv/odooscan/issues/257)) ([68986ff](https://github.com/avanserv/odooscan/commit/68986ffd4c4f840dd856ff60759720c1217cf847))
* **taint:** Keep OD707 silent on a joined path only file_open() opens ([#290](https://github.com/avanserv/odooscan/issues/290)) ([9e480f7](https://github.com/avanserv/odooscan/commit/9e480f7de79313a5d158355ea1cd6edc00743f95))
* **taint:** Keep the user side of a Domain joined with & ([#267](https://github.com/avanserv/odooscan/issues/267)) ([a162493](https://github.com/avanserv/odooscan/commit/a16249326124322241a2c78bcce357c7158fb56a))
* **taint:** Match the OD717 grant to route auth and ir.access domain ([#293](https://github.com/avanserv/odooscan/issues/293)) ([e09cf38](https://github.com/avanserv/odooscan/commit/e09cf3863cef0372ca94984e18cbeaa6d5c9ffd5))
* **taint:** Name the header a bound list sets in OD711 ([#265](https://github.com/avanserv/odooscan/issues/265)) ([f9b6b93](https://github.com/avanserv/odooscan/commit/f9b6b93d78a7d9291d3edb51b9df4c1fec7956f8))
* **taint:** Read &lt;&gt;, == and access as value operators in OD715 ([#285](https://github.com/avanserv/odooscan/issues/285)) ([9b66356](https://github.com/avanserv/odooscan/commit/9b66356933f281d5dfd7dc0aa7b57c82c44c1260))
* **taint:** Read env calls and with_user(False) in the sudo state ([#277](https://github.com/avanserv/odooscan/issues/277)) ([d9a8bf2](https://github.com/avanserv/odooscan/commit/d9a8bf25633f45c63e47e3b69bdd4b5d83be8255))
* **taint:** Read OD717 auth as unknown under an unread base ([#294](https://github.com/avanserv/odooscan/issues/294)) ([1d8d05c](https://github.com/avanserv/odooscan/commit/1d8d05c9d26648131b6b104d764fbb25c9d4c815))
* **taint:** Read superuser environments and the calls that leave sudo() ([#253](https://github.com/avanserv/odooscan/issues/253)) ([88f7f70](https://github.com/avanserv/odooscan/commit/88f7f70db3762581e65eb979a878ff5145ea226f))
* **taint:** Report a file_open() join a nested def or match reads ([#292](https://github.com/avanserv/odooscan/issues/292)) ([bf4e562](https://github.com/avanserv/odooscan/commit/bf4e5627ba7f80af22240ecb577b88e0cd3cb106))
* **taint:** Report io.open() in OD707 and say what the rows match ([#260](https://github.com/avanserv/odooscan/issues/260)) ([2e91749](https://github.com/avanserv/odooscan/commit/2e91749ba0666c203de98e0c514f66671e2aff50))
* **taint:** Report request.redirect(t) on 15.0, whose local keeps a backslash ([#289](https://github.com/avanserv/odooscan/issues/289)) ([3224686](https://github.com/avanserv/odooscan/commit/32246860bf5a349234a8087feced0d7a1fdf4674))
* **taint:** Say what a configured pattern ending in an item matches ([#252](https://github.com/avanserv/odooscan/issues/252)) ([e8c0b84](https://github.com/avanserv/odooscan/commit/e8c0b848286b762e34df36463f05ae45cde8f5fa))
* **taint:** Skip OD711 for headers of a bound requests or httpx client ([#264](https://github.com/avanserv/odooscan/issues/264)) ([7759100](https://github.com/avanserv/odooscan/commit/7759100c912e990ac6825058103ac0393835759c))
* **taint:** Stay silent in OD707 on upload names and checked paths ([#256](https://github.com/avanserv/odooscan/issues/256)) ([cee3e0b](https://github.com/avanserv/odooscan/commit/cee3e0b24b48184b2229f248b31113ead4ce44ad))
* **taint:** Track OD717 values per element and per key ([#271](https://github.com/avanserv/odooscan/issues/271)) ([372ff85](https://github.com/avanserv/odooscan/commit/372ff85ca67a160a8cbed1e4d2f04cdebcc584a0))
* **taint:** Trust only private _get_ methods as chosen in an if-in arm ([#286](https://github.com/avanserv/odooscan/issues/286)) ([4ef0deb](https://github.com/avanserv/odooscan/commit/4ef0deb73acf547e35a9238ac4dd3be8c7e185bf))
* **taint:** Vouch in OD716 only for checked records that passed ([#270](https://github.com/avanserv/odooscan/issues/270)) ([16b8a0c](https://github.com/avanserv/odooscan/commit/16b8a0cf9c4b14a3bc71e5eb13f4dca1b19e29f0))

## [0.10.0](https://github.com/avanserv/odooscan/compare/v0.9.0...v0.10.0) (2026-11-04)


### Features

* **checks:** Report asset declarations that later series no longer load ([#220](https://github.com/avanserv/odooscan/issues/220)) ([ecd2270](https://github.com/avanserv/odooscan/commit/ecd2270dc7ded8681daf75d446d1b5738f8976fe))
* **checks:** Report hook methods that nothing defines or names ([#224](https://github.com/avanserv/odooscan/issues/224)) ([7ca7959](https://github.com/avanserv/odooscan/commit/7ca79594a893e1f44333949c2c9a12b90912076c))
* **checks:** Report overrides whose signature no longer fits the parent method ([#223](https://github.com/avanserv/odooscan/issues/223)) ([b9b2f9c](https://github.com/avanserv/odooscan/commit/b9b2f9c9a78289577618133e200976e764d3e402))
* **checks:** Rewrite tree to list and t-esc to t-out with --fix ([#228](https://github.com/avanserv/odooscan/issues/228)) ([50780a1](https://github.com/avanserv/odooscan/commit/50780a1fa336f9f8f159282c31af71fd936e6256))
* **cli:** Check modules as of a target series and summarize the upgrade ([#225](https://github.com/avanserv/odooscan/issues/225)) ([623829d](https://github.com/avanserv/odooscan/commit/623829d8d29acde6dd8bbeb85ce2d7cbf72a37c3))
* **cli:** Fetch the core of the target series when none is given ([#226](https://github.com/avanserv/odooscan/issues/226)) ([9322d2c](https://github.com/avanserv/odooscan/commit/9322d2cece228217e8fd34c03f69ab7f80c6f789))
* **cli:** Rewrite mechanical Python deprecations with --fix and --diff ([#227](https://github.com/avanserv/odooscan/issues/227)) ([0ccf269](https://github.com/avanserv/odooscan/commit/0ccf269ddc29a2f1d85b8bd84734f5ff972b8770))
* **python:** Report deprecated and removed Python APIs per series ([#221](https://github.com/avanserv/odooscan/issues/221)) ([ad81dab](https://github.com/avanserv/odooscan/commit/ad81dabc32f9db39fe38f1ead5d79f4b244a1d60))
* **python:** Report removed context, onchange, override and cron APIs ([#222](https://github.com/avanserv/odooscan/issues/222)) ([6238ff3](https://github.com/avanserv/odooscan/commit/6238ff3d563f1cff018c16e1ea1dac27218ff9a8))
* **xml:** Report t-esc, kanban-box and oe_chatter that later drop ([#219](https://github.com/avanserv/odooscan/issues/219)) ([7e2afc3](https://github.com/avanserv/odooscan/commit/7e2afc3fca768dfb8aebf71fcf6a9c50688a62f5))


### Bug Fixes

* **checks:** Keep a t-esc unfixed beside a t-out add or remove spec ([#237](https://github.com/avanserv/odooscan/issues/237)) ([d24fc41](https://github.com/avanserv/odooscan/commit/d24fc4116cfc664d020f2357cb5aa65f21da5199))
* **checks:** Leave a tree or t-esc unfixed while a spec still finds it by its old name ([#236](https://github.com/avanserv/odooscan/issues/236)) ([aed854c](https://github.com/avanserv/odooscan/commit/aed854c204376e22503ad1eab150934f78e22bec))
* **checks:** Report forms Odoo ignores as OD125 warnings ([#240](https://github.com/avanserv/odooscan/issues/240)) ([7eacfda](https://github.com/avanserv/odooscan/commit/7eacfda0b120057c7450816f9bd047db80178d2d))
* **cli:** Keep a fixed file's group and leave out of --diff what --fix skips ([#235](https://github.com/avanserv/odooscan/issues/235)) ([aa29e8a](https://github.com/avanserv/odooscan/commit/aa29e8a754109afdbde5f943443999b7d971421e))
* **cli:** Kill a timed-out core fetch tree and word its failures ([#234](https://github.com/avanserv/odooscan/issues/234)) ([ba446ab](https://github.com/avanserv/odooscan/commit/ba446ab25458b9a95170c1a92f5deba290afc526))
* **index:** Compare a __name override with the classes of its class name ([#231](https://github.com/avanserv/odooscan/issues/231)) ([a9da6f0](https://github.com/avanserv/odooscan/commit/a9da6f03b2a06d8846c6349d9286221ac729a861))
* **index:** Read the methods of a plain base class defined above in its file ([#233](https://github.com/avanserv/odooscan/issues/233)) ([dd8330e](https://github.com/avanserv/odooscan/commit/dd8330ebd18b11027a72cf477a8ffe1583953fa6))
* **python:** Report a read_group override once and advise dropping numbercall -1 ([#230](https://github.com/avanserv/odooscan/issues/230)) ([6f37ce9](https://github.com/avanserv/odooscan/commit/6f37ce9643af65a79b7df64962b06600bd17876c))
* **rules:** Count a retyped or required core field as missing upstream ([#241](https://github.com/avanserv/odooscan/issues/241)) ([77e0842](https://github.com/avanserv/odooscan/commit/77e084208f1087983ebfaadf467fc44d9060d0cb))
* **rules:** Report OD306 as an error on 18.0 and later ([#218](https://github.com/avanserv/odooscan/issues/218)) ([438984a](https://github.com/avanserv/odooscan/commit/438984a24104f1990e06573ed6c86e324dae7761))

## [0.9.0](https://github.com/avanserv/odooscan/compare/v0.8.1...v0.9.0) (2026-11-03)


### Features

* **checks:** Check record rule domains and access rows without a group ([#110](https://github.com/avanserv/odooscan/issues/110)) ([844614c](https://github.com/avanserv/odooscan/commit/844614c3159f7cd800721960e363e95872b923fd))
* **checks:** Check repeated XML ids and required fields on create ([#114](https://github.com/avanserv/odooscan/issues/114)) ([03dc410](https://github.com/avanserv/odooscan/commit/03dc41058df2a1a8a29790fda001c00b4b88cf1d))
* **checks:** Check t-call targets in server QWeb and Owl templates ([#106](https://github.com/avanserv/odooscan/issues/106)) ([1e0b9d6](https://github.com/avanserv/odooscan/commit/1e0b9d6a2a30deeb7712019f567ec739c2022940))
* **checks:** Check the expressions of view modifiers ([#108](https://github.com/avanserv/odooscan/issues/108)) ([b465e62](https://github.com/avanserv/odooscan/commit/b465e62d66900d2e332dd3051c135bf7974c0a8f))
* **checks:** Check the methods buttons and data functions call ([#107](https://github.com/avanserv/odooscan/issues/107)) ([39d891e](https://github.com/avanserv/odooscan/commit/39d891e8f16d8b8cca4c3f3a26426bb5cb636465))
* **checks:** Check the model and the view types of window actions ([#109](https://github.com/avanserv/odooscan/issues/109)) ([312df45](https://github.com/avanserv/odooscan/commit/312df4551ab1e1c5e613edd912bae410f0790ee9))
* **checks:** Check the model of references and the values of Selection fields ([#113](https://github.com/avanserv/odooscan/issues/113)) ([a383297](https://github.com/avanserv/odooscan/commit/a383297dbcd83769f72b61aa1ea7fc75b187dc9d))
* **checks:** Check the syntax and the method calls of server code ([#111](https://github.com/avanserv/odooscan/issues/111)) ([60a1287](https://github.com/avanserv/odooscan/commit/60a128745e718390ce67b521ae10396b5ce52f6e))
* **checks:** Check the template and the file name of reports ([#112](https://github.com/avanserv/odooscan/issues/112)) ([e4ed331](https://github.com/avanserv/odooscan/commit/e4ed33141480031ab671d48dacc942f49b3acfd9))


### Bug Fixes

* **checks:** Align how far an unread data file silences rules ([#194](https://github.com/avanserv/odooscan/issues/194)) ([d664583](https://github.com/avanserv/odooscan/commit/d66458310b4bc108ec88cfd4f9b760b44f2599e0))
* **checks:** Correct OD323 and OD324 on series and NOT NULL ([#165](https://github.com/avanserv/odooscan/issues/165)) ([7a44ab2](https://github.com/avanserv/odooscan/commit/7a44ab2bb4a688f68ce3716eaf4b74a962449622))
* **checks:** Gate OD210 auto-installs by the reader's series ([#205](https://github.com/avanserv/odooscan/issues/205)) ([412a7c6](https://github.com/avanserv/odooscan/commit/412a7c6ec178370af77502741385cccd51f0c607))
* **checks:** Gate OD402 on transients and the ir.model.access series ([#155](https://github.com/avanserv/odooscan/issues/155)) ([413bba4](https://github.com/avanserv/odooscan/commit/413bba44ea69915d29a2ad7b761cbcd6b43b5f22))
* **checks:** Keep field ids of a table outside the scan silent ([#210](https://github.com/avanserv/odooscan/issues/210)) ([0189e36](https://github.com/avanserv/odooscan/commit/0189e36f22d661b9458c06813e8609447efebe14))
* **checks:** Keep OD217 and OD220 on a module of unknown series ([#204](https://github.com/avanserv/odooscan/issues/204)) ([f8fe323](https://github.com/avanserv/odooscan/commit/f8fe323275e1f71e08bc7a348f19fd199090e0a3))
* **checks:** Keep OD217 silent on an unknown series with parent_left ([#211](https://github.com/avanserv/odooscan/issues/211)) ([cb7856a](https://github.com/avanserv/odooscan/commit/cb7856a2d538d5ede475cee8d4e80f3cdf249ee4))
* **checks:** Name ir.access in OD401 from saas~19.4 ([#180](https://github.com/avanserv/odooscan/issues/180)) ([3cb7d40](https://github.com/avanserv/odooscan/commit/3cb7d400d70b3f97ec9713bdedc8ebad59412f3f))
* **checks:** Read a bridge's _date_name in OD318 ([#148](https://github.com/avanserv/odooscan/issues/148)) ([906e41c](https://github.com/avanserv/odooscan/commit/906e41c01cf99d0cfd27a2eb1bcb5f37ddafbfa3))
* **checks:** Read a child action's model from its parent in OD320 ([#158](https://github.com/avanserv/odooscan/issues/158)) ([be886b7](https://github.com/avanserv/odooscan/commit/be886b7f93913b82dfb7a86eb28086c66424e353))
* **checks:** Read an action's last view_id and the type it names ([#152](https://github.com/avanserv/odooscan/issues/152)) ([4db4f77](https://github.com/avanserv/odooscan/commit/4db4f772b1cba4ae79060bcc6305c02e481884bc))
* **checks:** Read an unset server action state as code below 11.0 ([#182](https://github.com/avanserv/odooscan/issues/182)) ([641ea04](https://github.com/avanserv/odooscan/commit/641ea0446358c7503b7a7d8ae078b4d8624aafdc))
* **checks:** Read auto-installs in the OD311 template lookup ([#207](https://github.com/avanserv/odooscan/issues/207)) ([15c4617](https://github.com/avanserv/odooscan/commit/15c4617b248f1372719687301cac7c27c14790d1))
* **checks:** Report a button on a private BaseModel method ([#126](https://github.com/avanserv/odooscan/issues/126)) ([77776bc](https://github.com/avanserv/odooscan/commit/77776bc3023f8ed3eb6b1e838be3dc0bb41759b5))
* **checks:** Report a name@fr field in OD309 below saas~18.2 ([#208](https://github.com/avanserv/odooscan/issues/208)) ([04f56ce](https://github.com/avanserv/odooscan/commit/04f56ceeff2fa474a509564361ad14af7cc820d1))
* **checks:** Report a record on a model outside depends once ([#104](https://github.com/avanserv/odooscan/issues/104)) ([d6e16bd](https://github.com/avanserv/odooscan/commit/d6e16bdc3d7629fc9e16a07752548ace43e8fa49))
* **checks:** Report activity in OD318 without mail in scope ([#147](https://github.com/avanserv/odooscan/issues/147)) ([4014f28](https://github.com/avanserv/odooscan/commit/4014f283d0bdcaa2a679f10ee92e266f1ab3fcb9))
* **checks:** Report an id behind a runtime check of another module ([#216](https://github.com/avanserv/odooscan/issues/216)) ([d95cfd8](https://github.com/avanserv/odooscan/commit/d95cfd89f48c8008b916d5609550e7d441fa29fc))
* **checks:** Report nothing from records a partial scan cannot judge ([#117](https://github.com/avanserv/odooscan/issues/117)) ([063b2cc](https://github.com/avanserv/odooscan/commit/063b2cc3c3f24f39ab9464b7d364f1f71dedc8c1))
* **checks:** Report OD319 only on code Odoo compiles or runs ([#197](https://github.com/avanserv/odooscan/issues/197)) ([cb4dd79](https://github.com/avanserv/odooscan/commit/cb4dd79b657c92107e8e94184afd6932f097fa06))
* **checks:** Report tuple selections and CSV case by series ([#162](https://github.com/avanserv/odooscan/issues/162)) ([f2bcabb](https://github.com/avanserv/odooscan/commit/f2bcabb8cbd62c3c064ff3763e31221ec05dc7a3))
* **checks:** Reword OD323 for a model clash and a menu's groups ([#164](https://github.com/avanserv/odooscan/issues/164)) ([802f36d](https://github.com/avanserv/odooscan/commit/802f36d6f273ba98ced5ec381dcc266f0c6ca296))
* **checks:** Stay silent in OD206 on a model the module may build ([#187](https://github.com/avanserv/odooscan/issues/187)) ([7d3cd50](https://github.com/avanserv/odooscan/commit/7d3cd504569d891083bab9d6c65e2367b4567462))
* **checks:** Stay silent in OD210 on a module the scope auto-installs ([#190](https://github.com/avanserv/odooscan/issues/190)) ([96db8c9](https://github.com/avanserv/odooscan/commit/96db8c979c98d924183ae7955ea6af59a02ef2e1))
* **checks:** Stay silent in OD210 on a parent the scan cannot read ([#154](https://github.com/avanserv/odooscan/issues/154)) ([2b9a778](https://github.com/avanserv/odooscan/commit/2b9a778db9a7acb168ebdd913cf07a913364f8db))
* **checks:** Stay silent in OD303 on a two-phase write ([#193](https://github.com/avanserv/odooscan/issues/193)) ([3d37180](https://github.com/avanserv/odooscan/commit/3d371800011ba1860b67d87d3d434620398acd95))
* **checks:** Stay silent in OD304 and OD210 on a model not seen whole ([#178](https://github.com/avanserv/odooscan/issues/178)) ([ed4002c](https://github.com/avanserv/odooscan/commit/ed4002c4c0b658a62262a4dc7fdd39079f258a63))
* **checks:** Stay silent in OD309 on ir.property value below 18.0 ([#203](https://github.com/avanserv/odooscan/issues/203)) ([e432bcf](https://github.com/avanserv/odooscan/commit/e432bcfde51f8822b50ff9f01a5f72a306f47efb))
* **checks:** Stay silent in OD311 on a template a dependency may load ([#195](https://github.com/avanserv/odooscan/issues/195)) ([4cd9d29](https://github.com/avanserv/odooscan/commit/4cd9d292520a4d40cb3ded7c3ec8ea31e7646bdd))
* **checks:** Stay silent in OD317 in an inline calendar subview ([#141](https://github.com/avanserv/odooscan/issues/141)) ([e497b3e](https://github.com/avanserv/odooscan/commit/e497b3eae1920094df56b0b39886a315ee42fc68))
* **checks:** Stay silent in OD320 on a call of env or pool ([#176](https://github.com/avanserv/odooscan/issues/176)) ([df8140e](https://github.com/avanserv/odooscan/commit/df8140eebd3a7a81c8d204caabb2244d9b3ff976))
* **checks:** Stay silent in OD323 on a template with forcecreate false ([#166](https://github.com/avanserv/odooscan/issues/166)) ([df02b3a](https://github.com/avanserv/odooscan/commit/df02b3a48d6dc2b6f7f5968f49e1e99429ac3c94))
* **checks:** Stay silent in OD604 on a plain base of the model ([#175](https://github.com/avanserv/odooscan/issues/175)) ([2a4b511](https://github.com/avanserv/odooscan/commit/2a4b51141df1af4e525e238c7fe34b4928aba9e8))
* **checks:** Stay silent on a hidden model the module builds at runtime ([#167](https://github.com/avanserv/odooscan/issues/167)) ([bd5c260](https://github.com/avanserv/odooscan/commit/bd5c260d7edbe8f17bbf7d76891d6820f5d0482f))
* **checks:** Stay silent on a partial scan in nine more rules ([#171](https://github.com/avanserv/odooscan/issues/171)) ([719fbf3](https://github.com/avanserv/odooscan/commit/719fbf34fc31fc6a1e828b833f349a76c0e55725))
* **checks:** Stop a delegate's plain base from hiding a missing method ([#125](https://github.com/avanserv/odooscan/issues/125)) ([4ca4fd1](https://github.com/avanserv/odooscan/commit/4ca4fd1dd308e324c4a290375e4afcbc227d25ba))
* **checks:** Stop a delegate's plain base hiding a missing view type ([#174](https://github.com/avanserv/odooscan/issues/174)) ([987c203](https://github.com/avanserv/odooscan/commit/987c2030ba074e029dda15c8e61f999212a72476))
* **decode:** Read a data CSV as ustr does below 11.0 ([#192](https://github.com/avanserv/odooscan/issues/192)) ([5370645](https://github.com/avanserv/odooscan/commit/5370645acd6bf6d3b43bade4d88eb346cbb07f7b))
* **index:** Count _neutralize and _get_access_action of BaseModel ([#137](https://github.com/avanserv/odooscan/issues/137)) ([1b17414](https://github.com/avanserv/odooscan/commit/1b17414f898a42f2026fa3ae0ad4c10ab4d792b7))
* **index:** Count a field an ir.model.fields record adds ([#134](https://github.com/avanserv/odooscan/issues/134)) ([2ce4849](https://github.com/avanserv/odooscan/commit/2ce4849cfb934298f2d791268f695186b2baf9ec))
* **index:** Count BaseModel and object dunders as parents of super() ([#136](https://github.com/avanserv/odooscan/issues/136)) ([bb4ccea](https://github.com/avanserv/odooscan/commit/bb4ccea16c37af5e221632e25c400189e4859499))
* **index:** Count every BaseModel method as a parent of super() ([#135](https://github.com/avanserv/odooscan/issues/135)) ([838ee17](https://github.com/avanserv/odooscan/commit/838ee172c5e6861d9cfadaa69a4098536e98f5b3))
* **index:** Count only primary views in OD318 ([#145](https://github.com/avanserv/odooscan/issues/145)) ([7768af7](https://github.com/avanserv/odooscan/commit/7768af716b31dbcbaa23f861f8d619bd8ef3d6a1))
* **index:** Count public BaseModel methods in OD604 by series ([#177](https://github.com/avanserv/odooscan/issues/177)) ([828be01](https://github.com/avanserv/odooscan/commit/828be0100a71406219a1b6b783d86fdacd361a41))
* **index:** Count the BaseModel methods gone by saas~11.5, by series ([#138](https://github.com/avanserv/odooscan/issues/138)) ([51125c6](https://github.com/avanserv/odooscan/commit/51125c6c572ac06aaf321ead598ec4dbda94e498))
* **index:** Count the class-level aliases of BaseModel as its methods ([#128](https://github.com/avanserv/odooscan/issues/128)) ([793c89f](https://github.com/avanserv/odooscan/commit/793c89f100886deb89c93cbdfbc0e05464ec3a05))
* **index:** End field lookups on shadowed fields and related cycles ([#115](https://github.com/avanserv/odooscan/issues/115)) ([d0be803](https://github.com/avanserv/odooscan/commit/d0be803cf526bb901ddf5d9348690ee9401ae42a))
* **index:** Give a model the XML ids of its inherited fields ([#209](https://github.com/avanserv/odooscan/issues/209)) ([afb5130](https://github.com/avanserv/odooscan/commit/afb51309c8fda941533b72a41fd6b19bc60a1848))
* **index:** Index views and keys only from the files Odoo loads ([#121](https://github.com/avanserv/odooscan/issues/121)) ([4411480](https://github.com/avanserv/odooscan/commit/441148015e506dad363278238c47eee7eb517191))
* **index:** Read only loaded data files for access and derived ids ([#172](https://github.com/avanserv/odooscan/issues/172)) ([80438bb](https://github.com/avanserv/odooscan/commit/80438bb21a774cb7015f1dd77b77585eb1b39978))
* **index:** Spell a field's XML id by table before saas~11.2 ([#199](https://github.com/avanserv/odooscan/issues/199)) ([c7eae57](https://github.com/avanserv/odooscan/commit/c7eae57ff571a655f7c0532e0b9c766ad4d58ddc))
* **index:** Stay silent in OD318 on a clone of an unindexed parent ([#146](https://github.com/avanserv/odooscan/issues/146)) ([2952588](https://github.com/avanserv/odooscan/commit/2952588e35d5ebce064dcade308761fdd35ad49d))
* **index:** Stay silent in OD321 on a comodel's unknown table ([#163](https://github.com/avanserv/odooscan/issues/163)) ([6b56328](https://github.com/avanserv/odooscan/commit/6b56328f443952bb125542e80c336146e3570bd6))
* **index:** Stop a demo write replacing a data value ([#150](https://github.com/avanserv/odooscan/issues/150)) ([ca12d2f](https://github.com/avanserv/odooscan/commit/ca12d2fa303e390257da5419da4b4d66a7fdf735))
* **modifier:** Substitute ids before %% as the data loader does ([#143](https://github.com/avanserv/odooscan/issues/143)) ([70113df](https://github.com/avanserv/odooscan/commit/70113dfabf1c00e338a7f14162abd2419c88b618))
* **project:** Read XML data files by their declared encoding ([#170](https://github.com/avanserv/odooscan/issues/170)) ([e5e736b](https://github.com/avanserv/odooscan/commit/e5e736b606c79ab0d27f25d2f7bd80f5e0a253f7))
* **python:** Count a field declared through a Field subclass ([#168](https://github.com/avanserv/odooscan/issues/168)) ([70a22d1](https://github.com/avanserv/odooscan/commit/70a22d1a36b107a2434af6a759c54229ba01fc1f))
* **python:** Give a notice for Python 2 syntax on 9.0 and 10.0 ([#214](https://github.com/avanserv/odooscan/issues/214)) ([5d44f13](https://github.com/avanserv/odooscan/commit/5d44f13a6dd67a1495ad66cdf0aaec390cf9d634))
* **python:** Index the models a file that fails to parse names ([#184](https://github.com/avanserv/odooscan/issues/184)) ([0ec35b2](https://github.com/avanserv/odooscan/commit/0ec35b2634e3c007189bc1ce756c80ab915e26e4))
* **python:** Read a call alias and a conditional def as a method ([#127](https://github.com/avanserv/odooscan/issues/127)) ([b5dd745](https://github.com/avanserv/odooscan/commit/b5dd745f9f683d9134827d1c043a9e080b91e1b1))
* **python:** Read async and await as names below 15.0 ([#181](https://github.com/avanserv/odooscan/issues/181)) ([6cd33f5](https://github.com/avanserv/odooscan/commit/6cd33f5e72857ce6581ac7fe4fae9d585729b65a))
* **python:** Read ensure_installed as a runtime guard ([#206](https://github.com/avanserv/odooscan/issues/206)) ([110dbdd](https://github.com/avanserv/odooscan/commit/110dbddb2314b78e29283f65460b0a16883d3c6b))
* **python:** Read osv.osv and its aliases as model bases ([#189](https://github.com/avanserv/odooscan/issues/189)) ([fd3597e](https://github.com/avanserv/odooscan/commit/fd3597e8e6f60d1ae37b171a5478c1480f42811b))
* **python:** Read the keys of a literal _columns as fields ([#188](https://github.com/avanserv/odooscan/issues/188)) ([9e56086](https://github.com/avanserv/odooscan/commit/9e56086627ab26fde28813ac5141d3ada88c4525))
* **python:** Stay silent behind a runtime guard of a soft dependency ([#169](https://github.com/avanserv/odooscan/issues/169)) ([d5cc40b](https://github.com/avanserv/odooscan/commit/d5cc40b38f788e749d0e75bd90e4f8eb89a45f90))
* **python:** Stay silent in OD211 on a super() call behind hasattr ([#140](https://github.com/avanserv/odooscan/issues/140)) ([ca81c48](https://github.com/avanserv/odooscan/commit/ca81c4816366affe394683de2b4e278f6329a3e0))
* **python:** Stay silent in OD302 behind a runtime guard ([#191](https://github.com/avanserv/odooscan/issues/191)) ([9d19dab](https://github.com/avanserv/odooscan/commit/9d19dab838b7351e82b6c4432ef7a77f5df4e3d4))
* **python:** Stop counting super() calls of a class nested in a method ([#139](https://github.com/avanserv/odooscan/issues/139)) ([7551dc6](https://github.com/avanserv/odooscan/commit/7551dc6f9ce7352515e575c9a6189a578309aea6))
* **rules:** Correct the series gates of six rules from Odoo source ([#105](https://github.com/avanserv/odooscan/issues/105)) ([846f7ca](https://github.com/avanserv/odooscan/commit/846f7caf8747aa42fac34b617dfbe8cf7045f2bc))
* **rules:** Report OD401 as Info from saas~19.4 ([#153](https://github.com/avanserv/odooscan/issues/153)) ([5d9cb63](https://github.com/avanserv/odooscan/commit/5d9cb63954d31c196818d7089c6b0c33fa28234e))
* **server_code:** Read async and await as names below 15.0 ([#157](https://github.com/avanserv/odooscan/issues/157)) ([ed580fb](https://github.com/avanserv/odooscan/commit/ed580fb4f2a7f0a17e7fc8e8a068e1797f4d77a2))
* **xml:** Compare modifier values to the constants as Odoo does ([#142](https://github.com/avanserv/odooscan/issues/142)) ([4d5d816](https://github.com/avanserv/odooscan/commit/4d5d816df70124e08bfdfafb391655d96b916afc))
* **xml:** Count a view_ids row only with a view, nested from saas~15.1 ([#151](https://github.com/avanserv/odooscan/issues/151)) ([10de4c2](https://github.com/avanserv/odooscan/commit/10de4c2e9407d166bc4da7f31ec1cc09bc86ce22))
* **xml:** Count source positions in code_pos for entities and CDATA ([#159](https://github.com/avanserv/odooscan/issues/159)) ([2f8b5f3](https://github.com/avanserv/odooscan/commit/2f8b5f38bae22c34081317fd2d63a797a9b19880))
* **xml:** Name a template without id by its t-name ([#119](https://github.com/avanserv/odooscan/issues/119)) ([740a3d2](https://github.com/avanserv/odooscan/commit/740a3d230b4e2dcae083e101fbb2fdc48643326e))
* **xml:** Read a subview a spec adds on its field's comodel ([#133](https://github.com/avanserv/odooscan/issues/133)) ([94939da](https://github.com/avanserv/odooscan/commit/94939da5abf50097517588af3ad907f8d9b94800))
* **xml:** Read a t-call on a spec as a locator ([#122](https://github.com/avanserv/odooscan/issues/122)) ([2b420f4](https://github.com/avanserv/odooscan/commit/2b420f43052fb10c852777ceaf19d778a7427767))
* **xml:** Read act_window_id by eval and view_id by search in OD318 ([#144](https://github.com/avanserv/odooscan/issues/144)) ([9f1e89b](https://github.com/avanserv/odooscan/commit/9f1e89b4f6ed35d82d99249987d314247444c5f3))
* **xml:** Skip the root t-call of an extending template ([#120](https://github.com/avanserv/odooscan/issues/120)) ([ecd5862](https://github.com/avanserv/odooscan/commit/ecd58626955393f704df95b80b68d9f27e2bf932))

## [0.8.1](https://github.com/avanserv/odooscan/compare/v0.8.0...v0.8.1) (2026-10-30)


### Bug Fixes

* **cli:** Expand ~ in addons paths and reject a missing one ([#102](https://github.com/avanserv/odooscan/issues/102)) ([623f3b2](https://github.com/avanserv/odooscan/commit/623f3b2867f401279d157f291648189aa3e59c4a))

## [0.8.0](https://github.com/avanserv/odooscan/compare/v0.7.0...v0.8.0) (2026-10-30)


### Features

* **checks:** Report a delegate=True field with no ondelete as OD228 ([#101](https://github.com/avanserv/odooscan/issues/101)) ([93c3860](https://github.com/avanserv/odooscan/commit/93c3860fdc1a1cbbdf8750fbcc8fe2768cea92e5))
* **checks:** Report field definitions Odoo refuses at registry setup ([#48](https://github.com/avanserv/odooscan/issues/48)) ([00bf2bc](https://github.com/avanserv/odooscan/commit/00bf2bca92f0941eceaf3ecea3af64254030c2ed))
* **checks:** Report field options Odoo ignores or logs ([#50](https://github.com/avanserv/odooscan/issues/50)) ([1209945](https://github.com/avanserv/odooscan/commit/1209945971d5a286182c7127fd2407fbca296e41))
* **checks:** Report model attributes Odoo logs or ignores ([#49](https://github.com/avanserv/odooscan/issues/49)) ([52657db](https://github.com/avanserv/odooscan/commit/52657db9f0d8b199aced0bd6cfb49ae7d9d3777d))
* **checks:** Report SQL columns that no stored field provides ([#53](https://github.com/avanserv/odooscan/issues/53)) ([e9918f4](https://github.com/avanserv/odooscan/commit/e9918f48037f3eaf61727dd4e3eeb4e1872567a2))
* **checks:** Report SQL views and constraints that miss field columns ([#54](https://github.com/avanserv/odooscan/issues/54)) ([b761e45](https://github.com/avanserv/odooscan/commit/b761e456f1919ecfbb98cae5bca13d50ef4c2959))
* **python:** Report ORM overrides and caches that fail at runtime ([#51](https://github.com/avanserv/odooscan/issues/51)) ([15a6c3d](https://github.com/avanserv/odooscan/commit/15a6c3d23ba809a5b5239f7775dc0e474988db26))
* **python:** Report rollbacks, raw SQL writes and import-time state ([#52](https://github.com/avanserv/odooscan/issues/52)) ([4cefa44](https://github.com/avanserv/odooscan/commit/4cefa440e489fc55113508bfb8363869d15b2c90))


### Bug Fixes

* **checks:** Call _table_query an attribute of BaseModel in OD116 ([#80](https://github.com/avanserv/odooscan/issues/80)) ([7d38486](https://github.com/avanserv/odooscan/commit/7d38486024a0b1a8be41a87e68b4da49daf6d30e))
* **checks:** Check a related field's model_field in OD203 ([#62](https://github.com/avanserv/odooscan/issues/62)) ([0260f74](https://github.com/avanserv/odooscan/commit/0260f74749eff94195a6486b649e13619a21a04f))
* **checks:** Close OD223 gaps on continue and guarded loops ([#81](https://github.com/avanserv/odooscan/issues/81)) ([8563fa0](https://github.com/avanserv/odooscan/commit/8563fa00721e8040063bc05d1a5529fe0fc429b8))
* **checks:** Count a filling endless while in the OD223 exit scan ([#94](https://github.com/avanserv/odooscan/issues/94)) ([48ae499](https://github.com/avanserv/odooscan/commit/48ae499c24981c932bdd650e747112510fb22f1b))
* **checks:** Date _condition_to_sql and _table_query in OD116 by release ([#63](https://github.com/avanserv/odooscan/issues/63)) ([3ae9bbf](https://github.com/avanserv/odooscan/commit/3ae9bbfaa000116be429c864cf78057f9790a9db))
* **checks:** Date the OD220 magic-field rules by series ([#64](https://github.com/avanserv/odooscan/issues/64)) ([9a1e616](https://github.com/avanserv/odooscan/commit/9a1e616a77257c50f7bf9e0d63cfa8cf541e2f91))
* **checks:** Drop the base clause of the _inherits order in OD224 ([#91](https://github.com/avanserv/odooscan/issues/91)) ([f9093a3](https://github.com/avanserv/odooscan/commit/f9093a3c0f1cee8f959e2670ce0ecac981597ab2))
* **checks:** Explain OD225 on a dependency field, share query lookup ([#76](https://github.com/avanserv/odooscan/issues/76)) ([f73c6eb](https://github.com/avanserv/odooscan/commit/f73c6ebeaa099f71c6aab45e64d91cfb780b8d41))
* **checks:** Fix OD224 on comments, psycopg2 SQL and log columns ([#73](https://github.com/avanserv/odooscan/issues/73)) ([c49be3b](https://github.com/avanserv/odooscan/commit/c49be3b6437dabcff0d91bd678c181f9cede96e3))
* **checks:** Fix OD226 manifest keys, view names and any-view ([#75](https://github.com/avanserv/odooscan/issues/75)) ([0f250d4](https://github.com/avanserv/odooscan/commit/0f250d46befaba83085c40dc004290b346d02428))
* **checks:** Let a selection list replace earlier values in OD216 ([#59](https://github.com/avanserv/odooscan/issues/59)) ([65a5745](https://github.com/avanserv/odooscan/commit/65a574549af55f48ae51c07c68db21377c38bacf))
* **checks:** Name a shorter relation as the fix in every OD215 message ([#60](https://github.com/avanserv/odooscan/issues/60)) ([45f74c5](https://github.com/avanserv/odooscan/commit/45f74c5ac0b70cbb4d01e2257b9e7f58a3ecdb76))
* **checks:** Order the parents of one _inherit in OD224 and OD227 ([#86](https://github.com/avanserv/odooscan/issues/86)) ([35d371e](https://github.com/avanserv/odooscan/commit/35d371e09fb44142b34e0fb15c9280d161cc630b))
* **checks:** Read _inherits as unknown past a non-literal _inherit ([#93](https://github.com/avanserv/odooscan/issues/93)) ([6fcb897](https://github.com/avanserv/odooscan/commit/6fcb89725a0056c0cbf4b8500da18b7528b1f909))
* **checks:** Read an endless while in OD223 without its continue paths ([#90](https://github.com/avanserv/odooscan/issues/90)) ([adfc2d9](https://github.com/avanserv/odooscan/commit/adfc2d940240c3a1ce37e2d55326c2bb7398e0b2))
* **checks:** Read inherited _inherits and the ondelete rewrite ([#61](https://github.com/avanserv/odooscan/issues/61)) ([e052209](https://github.com/avanserv/odooscan/commit/e0522092e774aefbcc5ae094d9fd27c784d7b8a7))
* **checks:** Read inherited _inherits entries in OD224 and OD227 ([#77](https://github.com/avanserv/odooscan/issues/77)) ([ca3af31](https://github.com/avanserv/odooscan/commit/ca3af31013e31f95fc7d0debe4fc5817438c65ba))
* **checks:** Read mixin magic fields in OD220 as Odoo merges them ([#79](https://github.com/avanserv/odooscan/issues/79)) ([dab5047](https://github.com/avanserv/odooscan/commit/dab504792bdddf6d689ba1c3e21d0841e3642f49))
* **checks:** Skip a model with a non-literal _inherit like a plain base ([#96](https://github.com/avanserv/odooscan/issues/96)) ([0abaa81](https://github.com/avanserv/odooscan/commit/0abaa81c178b65c2aa0983993589b43106958c1d))
* **checks:** Skip classes with _register = False in OD401 ([#74](https://github.com/avanserv/odooscan/issues/74)) ([c2dbb27](https://github.com/avanserv/odooscan/commit/c2dbb2701de626573dc9298d53550ef1d113877e))
* **checks:** Stop OD204 counting methods of _inherits parents ([#78](https://github.com/avanserv/odooscan/issues/78)) ([4fcd730](https://github.com/avanserv/odooscan/commit/4fcd730ad9596ff71780395dbd0e7d562a2866c7))
* **checks:** Stop OD211 counting methods of _inherits parents ([#89](https://github.com/avanserv/odooscan/issues/89)) ([71ad156](https://github.com/avanserv/odooscan/commit/71ad15613d92558aef8530ba4bd5c8806097fde9))
* **checks:** Stop OD223 and OD120 false positives ([#67](https://github.com/avanserv/odooscan/issues/67)) ([8348c83](https://github.com/avanserv/odooscan/commit/8348c83922d354dbd7db228e5c785ed1ad806078))
* **checks:** Stop OD604 counting methods of _inherits parents ([#88](https://github.com/avanserv/odooscan/issues/88)) ([0a03d3f](https://github.com/avanserv/odooscan/commit/0a03d3fc83c63893f53dd8c32283c6c81bfdb42c))
* **checks:** Tell model classes apart in OD123 advice by the index ([#71](https://github.com/avanserv/odooscan/issues/71)) ([1dfab82](https://github.com/avanserv/odooscan/commit/1dfab825459e4b7debf137b986b96e9b7c870922))
* **index:** Count always-installed modules as in depends ([#47](https://github.com/avanserv/odooscan/issues/47)) ([c4a696c](https://github.com/avanserv/odooscan/commit/c4a696cbb75502b41a6e16623381faeae0c34d7f))
* **index:** Count list-form auto_install modules as always installed ([#56](https://github.com/avanserv/odooscan/issues/56)) ([e41dc8d](https://github.com/avanserv/odooscan/commit/e41dc8d17222948120acd2e6fcc3bf8616fd1b40))
* **index:** Name the module that defines a model in OD202 ([#46](https://github.com/avanserv/odooscan/issues/46)) ([e6848df](https://github.com/avanserv/odooscan/commit/e6848dfb3b468fc2a60a5e09d681a06fd0b95d15))
* **manifest:** Load data files under legacy keys by series ([#85](https://github.com/avanserv/odooscan/issues/85)) ([b262633](https://github.com/avanserv/odooscan/commit/b262633bf9dfa54abd6df03e7cbc40db00badb1e))
* **method_body:** Report OD223 on unpaired writes and exits ([#66](https://github.com/avanserv/odooscan/issues/66)) ([bd98dd8](https://github.com/avanserv/odooscan/commit/bd98dd8efd9f16fd1c8275be08c7e06e672cc55d))
* **python:** Count models.CachedModel as a model base ([#72](https://github.com/avanserv/odooscan/issues/72)) ([a9e9720](https://github.com/avanserv/odooscan/commit/a9e97209518297af134d7b031049d264408a6681))
* **python:** Name a class without _name as Odoo does in its series ([#95](https://github.com/avanserv/odooscan/issues/95)) ([3b503b9](https://github.com/avanserv/odooscan/commit/3b503b950b08e158358d5dd1fff1a174cce4ce8e))
* **python:** Stop OD103 and OD121 on cursors the function opened ([#69](https://github.com/avanserv/odooscan/issues/69)) ([af09778](https://github.com/avanserv/odooscan/commit/af09778fc1f146e3233c223d1361047af1f00b43))
* **rules:** Report OD222 as an error from saas~16.4 ([#97](https://github.com/avanserv/odooscan/issues/97)) ([fcd973c](https://github.com/avanserv/odooscan/commit/fcd973c5bbe9dd686453fef50ccc51310c3e9706))
* **rules:** Report OD223 from saas~13.3, where Odoo starts raising ([#82](https://github.com/avanserv/odooscan/issues/82)) ([760fc39](https://github.com/avanserv/odooscan/commit/760fc3941bc200db9af17de02de8e2b73b9ee8e9))
* **sql:** Cut statements with the query lexer's literals ([#84](https://github.com/avanserv/odooscan/issues/84)) ([bc76ee2](https://github.com/avanserv/odooscan/commit/bc76ee233a532a10da583577508fd385e598904c))
* **sql:** End a cursor scan path at a block whose every branch leaves ([#83](https://github.com/avanserv/odooscan/issues/83)) ([8e3e810](https://github.com/avanserv/odooscan/commit/8e3e810e673eed16648d8ff3af767936f75a74cd))
* **sql:** Track own cursors by path and limit create_column to UPDATE ([#70](https://github.com/avanserv/odooscan/issues/70)) ([8bfddeb](https://github.com/avanserv/odooscan/commit/8bfddeb925d03903fa0c375aa5e02c75027df8dd))

## [0.7.0](https://github.com/avanserv/odooscan/compare/v0.6.0...v0.7.0) (2026-10-28)


### Features

* **checks:** Follow tainted data across model methods ([#43](https://github.com/avanserv/odooscan/issues/43)) ([5d311cb](https://github.com/avanserv/odooscan/commit/5d311cbb16dcab6bb175c17b983fb80d98f02c70))
* **taint:** Report request data that reaches SQL, eval, Markup or a sudo() write ([#42](https://github.com/avanserv/odooscan/issues/42)) ([b8925c0](https://github.com/avanserv/odooscan/commit/b8925c0cf98e5e72d1bf051d5ec410ba2197a37d))

## [0.6.0](https://github.com/avanserv/odooscan/compare/v0.5.0...v0.6.0) (2026-10-28)


### Features

* **checks:** Check the models and methods of ORM calls ([#37](https://github.com/avanserv/odooscan/issues/37)) ([1b1338f](https://github.com/avanserv/odooscan/commit/1b1338fe4e25f961b2c4c9fe13c710ab637df823))
* **checks:** Report a patch whose super call has no parent method ([#36](https://github.com/avanserv/odooscan/issues/36)) ([704ac3b](https://github.com/avanserv/odooscan/commit/704ac3b2338a95231ce0d5b258cd9ab9db4bd531))
* **js:** Index JS modules and report unresolved imports ([#35](https://github.com/avanserv/odooscan/issues/35)) ([317f897](https://github.com/avanserv/odooscan/commit/317f8979fac0d5f7702461b0e02f784213d3177b))
* **js:** Report an RPC call to a private method ([#41](https://github.com/avanserv/odooscan/issues/41)) ([59df3fe](https://github.com/avanserv/odooscan/commit/59df3fe140e26d67fe63ec01393cc94c1fc5af86))
* **owl:** Check Owl template names and extension locators ([#40](https://github.com/avanserv/odooscan/issues/40)) ([79d2ccf](https://github.com/avanserv/odooscan/commit/79d2ccf48bb6bf1d1ad17bbaa779b2329e5f2593))

## [0.5.0](https://github.com/avanserv/odooscan/compare/v0.4.0...v0.5.0) (2026-10-28)


### Features

* **cli:** Print a rule's documentation with oscn explain ([#29](https://github.com/avanserv/odooscan/issues/29)) ([20084af](https://github.com/avanserv/odooscan/commit/20084afa6aec5a11a84184f09e91827c4f2f6ec6))
* **output:** Link each finding to its rule page ([#31](https://github.com/avanserv/odooscan/issues/31)) ([9df56e8](https://github.com/avanserv/odooscan/commit/9df56e8c20942b36e7484fc4e5ad9cc85321dd55))

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
