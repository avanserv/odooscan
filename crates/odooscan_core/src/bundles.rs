//! The files each asset bundle loads, filled as Odoo fills one from the manifests of the installed
//! modules (18.0 `odoo/addons/base/models/ir_asset.py:158-241`, `:391-430`; 20.0 `:151-237`,
//! `:381-420`). Where Odoo raises and fails the bundle, the fill goes on as described at each step,
//! so that one faulty entry does not hide every file of the bundle. Where Odoo skips an entry
//! without raising, so does the fill.

use rustc_hash::{FxHashMap, FxHashSet};

use crate::checks::wildcard;
use crate::index::asset_glob_set;
use crate::manifest::{AssetEntry, Directive};
use crate::project::{ASSET_EXTENSIONS, Module};

/// A file of an indexed module, as (module, index among the module's files of its kind).
pub type File = (usize, usize);

/// The indexed files a bundle loads, each kind in the bundle's order.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Contents {
    pub js: Vec<File>,
    pub templates: Vec<File>,
}

/// The contents of each bundle some manifest of `modules` names, from the modules' entries in
/// `order`, their load order.
pub fn of_modules(
    modules: &[Module],
    by_name: &FxHashMap<&str, usize>,
    order: &[usize],
) -> FxHashMap<String, Contents> {
    let mut lists: FxHashMap<&str, Vec<&[AssetEntry]>> = FxHashMap::default();
    for &mi in order {
        for bundle in &modules[mi].manifest.bundles {
            lists.entry(&bundle.name.value).or_default().push(&bundle.entries);
        }
    }
    let mut filler = Filler {
        lists,
        paths: Paths::of_modules(modules, by_name),
    };
    let names: Vec<&str> = filler.lists.keys().copied().collect();
    let mut contents = FxHashMap::default();
    for name in names {
        let mut filled = Filled::default();
        filler.fill(name, &mut filled, &mut vec![]);
        contents.insert(name.to_owned(), filler.paths.contents(&filled.list));
    }
    contents
}

/// What a file of the scan is.
#[derive(Clone, Copy)]
enum Kind {
    Js,
    Template,
}

/// The paths bundles name, each under a number: the indexed files first, at their place in
/// `files`, then any other path an entry names.
struct Paths<'m> {
    text: Vec<String>,
    ids: FxHashMap<String, usize>,
    files: Vec<(Kind, File)>,
    /// The numbers of each module's indexed files.
    by_module: Vec<Vec<usize>>,
    by_name: &'m FxHashMap<&'m str, usize>,
    modules: &'m [Module],
    /// What each path or glob an entry writes resolves to, so a glob is matched once.
    resolved: FxHashMap<&'m str, Vec<usize>>,
}

impl<'m> Paths<'m> {
    fn of_modules(modules: &'m [Module], by_name: &'m FxHashMap<&'m str, usize>) -> Self {
        let mut paths = Self {
            text: vec![],
            ids: FxHashMap::default(),
            files: vec![],
            by_module: vec![vec![]; modules.len()],
            by_name,
            modules,
            resolved: FxHashMap::default(),
        };
        for (mi, module) in modules.iter().enumerate() {
            let js = module.js.iter().map(|f| (Kind::Js, &f.rel));
            let templates = module.templates.iter().map(|f| (Kind::Template, &f.rel));
            let mut counts = [0, 0];
            for (kind, rel) in js.chain(templates) {
                let fi = counts[kind as usize];
                counts[kind as usize] += 1;
                let id = paths.intern(&format!("{}/{rel}", module.name));
                paths.files.push((kind, (mi, fi)));
                paths.by_module[mi].push(id);
            }
        }
        paths
    }

    fn intern(&mut self, path: &str) -> usize {
        if let Some(&id) = self.ids.get(path) {
            return id;
        }
        self.text.push(path.to_owned());
        self.ids.insert(path.to_owned(), self.text.len() - 1);
        self.text.len() - 1
    }

    /// The paths `path` stands for: a glob the files of the module its first segment names that
    /// it matches, sorted, and any other path itself, whether a file of the scan or not (`:47-49`,
    /// `:367`).
    fn resolve(&mut self, path: &'m str) -> Vec<usize> {
        if let Some(ids) = self.resolved.get(path) {
            return ids.clone();
        }
        let ids = if wildcard(path) {
            self.matches(path)
        } else {
            vec![self.intern(path)]
        };
        self.resolved.insert(path, ids.clone());
        ids
    }

    fn matches(&self, glob: &str) -> Vec<usize> {
        let module = glob.split('/').next().and_then(|name| self.by_name.get(name));
        let Some(&mi) = module else {
            return vec![];
        };
        let set = asset_glob_set([&glob.to_owned()]);
        let mut ids: Vec<usize> = self.by_module[mi]
            .iter()
            .copied()
            .filter(|&id| set.is_match(&self.text[id]))
            .collect();
        ids.sort_unstable_by(|&a, &b| self.text[a].cmp(&self.text[b]));
        ids
    }

    /// Whether Odoo skips an entry placed at `target`: a glob whose extension is no asset's and
    /// that matches no file under `static` of its module (`:217-221`); any other target stands for
    /// itself when nothing matches (`:367-368`). `static_assets` holds the styles too, which
    /// `files` leaves out. A module whose `static_assets` the scan does not read keeps the entry.
    fn skips(&self, target: &str) -> bool {
        let extension = target.rsplit('.').next().unwrap_or_default();
        if !wildcard(target) || ASSET_EXTENSIONS.contains(&extension) {
            return false;
        }
        let module = target.split('/').next().and_then(|name| self.by_name.get(name));
        let Some(&mi) = module else {
            return true;
        };
        let module = &self.modules[mi];
        let set = asset_glob_set([&target.to_owned()]);
        // A glob globset cannot read may match files, so the entry stays.
        module.reads_static_assets() && !set.is_empty() && !module.static_assets.iter().any(|f| set.is_match(f))
    }

    fn contents(&self, list: &[usize]) -> Contents {
        let mut contents = Contents::default();
        for &(kind, file) in list.iter().filter_map(|&id| self.files.get(id)) {
            match kind {
                Kind::Js => contents.js.push(file),
                Kind::Template => contents.templates.push(file),
            }
        }
        contents
    }
}

/// A bundle being filled: its paths in order, and the same as a set.
#[derive(Default)]
struct Filled {
    list: Vec<usize>,
    memo: FxHashSet<usize>,
}

impl Filled {
    /// Adds the paths not yet listed at `at`, or at the end.
    fn insert(&mut self, paths: &[usize], at: Option<usize>) {
        let new: Vec<usize> = paths.iter().copied().filter(|&p| self.memo.insert(p)).collect();
        // A remove in an included bundle can shorten the list below where it began.
        let at = at.map_or(self.list.len(), |at| at.min(self.list.len()));
        self.list.splice(at..at, new);
    }

    /// Drops the paths listed; Odoo raises when none is, which leaves nothing to drop here.
    fn remove(&mut self, paths: &[usize]) {
        let gone: FxHashSet<usize> = paths.iter().copied().filter(|p| self.memo.remove(p)).collect();
        if !gone.is_empty() {
            self.list.retain(|p| !gone.contains(p));
        }
    }

    /// Where the first path `target` stands for is listed.
    fn position(&self, target: &[usize]) -> Option<usize> {
        let first = target.first().filter(|t| self.memo.contains(t))?;
        self.list.iter().position(|p| p == first)
    }
}

struct Filler<'m> {
    /// The entries of each bundle, one list per module that names it, in load order.
    lists: FxHashMap<&'m str, Vec<&'m [AssetEntry]>>,
    paths: Paths<'m>,
}

impl<'m> Filler<'m> {
    /// Applies every entry for `bundle` to `filled`. A bundle already on the include path is
    /// skipped, where Odoo raises (`:172`).
    fn fill(&mut self, bundle: &'m str, filled: &mut Filled, seen: &mut Vec<&'m str>) {
        if seen.contains(&bundle) {
            return;
        }
        let Some(lists) = self.lists.get(bundle).cloned() else {
            return;
        };
        // `prepend` inserts at the start of the bundle being filled, an included one too (`:177`).
        let start = filled.list.len();
        seen.push(bundle);
        for entry in lists.into_iter().flatten() {
            self.apply(entry, filled, seen, start);
        }
        seen.pop();
    }

    fn apply(&mut self, entry: &'m AssetEntry, filled: &mut Filled, seen: &mut Vec<&'m str>, start: usize) {
        if entry.directive == Directive::Include {
            self.fill(&entry.path, filled, seen);
            return;
        }
        if entry.target.as_deref().is_some_and(|t| self.paths.skips(t)) {
            return;
        }
        let paths = self.paths.resolve(&entry.path);
        let target = entry
            .target
            .as_deref()
            .map(|t| self.paths.resolve(t))
            .unwrap_or_default();
        // A target that is not listed appends, where Odoo raises.
        let at = match entry.directive {
            Directive::Remove => return filled.remove(&paths),
            Directive::Prepend => Some(start),
            Directive::After => filled.position(&target).map(|i| i + 1),
            Directive::Before | Directive::Replace => filled.position(&target),
            Directive::Append | Directive::Include => None,
        };
        filled.insert(&paths, at);
        if entry.directive == Directive::Replace {
            filled.remove(&target);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::project::{SourceFile, module};

    fn file<T: Default>(rel: &str) -> SourceFile<T> {
        SourceFile {
            path: PathBuf::new(),
            rel: rel.to_owned(),
            rank: None,
            demo: false,
            facts: T::default(),
            suppressions: Default::default(),
            mentions: Default::default(),
        }
    }

    /// A module of the scripts `js` and the template files `xml`, under `static/src`, with the
    /// manifest keys `keys`.
    fn addon(name: &str, keys: &str, js: &[&str], xml: &[&str]) -> Module {
        let mut m = module(name, &format!("{{'version': '18.0.1.0', {keys}}}"));
        m.js = js.iter().map(|f| file(&format!("static/src/{f}"))).collect();
        m.templates = xml.iter().map(|f| file(&format!("static/src/{f}"))).collect();
        m
    }

    /// Each bundle's files as `<module>/<file under static/src>`, scripts then templates, of
    /// `modules` loaded in the order given.
    fn contents(modules: &[Module]) -> FxHashMap<String, Vec<String>> {
        let by_name = modules.iter().enumerate().map(|(i, m)| (m.name.as_str(), i)).collect();
        let order: Vec<usize> = (0..modules.len()).collect();
        let name = |&(mi, fi): &File, xml: bool| {
            let m: &Module = &modules[mi];
            let rel = if xml { &m.templates[fi].rel } else { &m.js[fi].rel };
            format!("{}/{}", m.name, rel.trim_start_matches("static/src/"))
        };
        let bundles = of_modules(modules, &by_name, &order);
        let files = |c: &Contents| -> Vec<String> {
            let js = c.js.iter().map(|f| name(f, false));
            js.chain(c.templates.iter().map(|f| name(f, true))).collect()
        };
        bundles.iter().map(|(k, c)| (k.clone(), files(c))).collect()
    }

    #[test]
    fn appends_a_file_once() {
        let a = addon(
            "a",
            r#""assets": {"b": ["a/static/src/*.js", "a/static/src/z.js", "a/static/src/**/*"]}"#,
            &["z.js", "y.js", "x/w.js"],
            &["t.xml"],
        );
        assert_eq!(contents(&[a])["b"], ["a/y.js", "a/z.js", "a/x/w.js", "a/t.xml"]);
    }

    #[test]
    fn prepends_at_the_start_of_an_included_bundle() {
        let a = addon(
            "a",
            r#""assets": {
                "outer": ["a/static/src/o.js", ("include", "inner")],
                "inner": ["a/static/src/i.js", ("prepend", "a/static/src/p.js")],
            }"#,
            &["o.js", "i.js", "p.js"],
            &[],
        );
        let bundles = contents(&[a]);
        assert_eq!(bundles["outer"], ["a/o.js", "a/p.js", "a/i.js"]);
        assert_eq!(bundles["inner"], ["a/p.js", "a/i.js"]);
    }

    #[test]
    fn places_before_and_after_the_first_match_of_the_target() {
        let a = addon(
            "a",
            r#""assets": {"b": [
                "a/static/src/m*.js",
                ("before", "a/static/src/m*.js", "a/static/src/b.js"),
                ("after", "/a/static/src/m1.js", "a/static/src/c.js"),
            ]}"#,
            &["m1.js", "m2.js", "b.js", "c.js"],
            &[],
        );
        assert_eq!(contents(&[a])["b"], ["a/b.js", "a/m1.js", "a/c.js", "a/m2.js"]);
    }

    #[test]
    fn skips_a_bundle_already_on_the_include_path() {
        let a = addon(
            "a",
            r#""assets": {
                "one": ["a/static/src/one.js", ("include", "two")],
                "two": ["a/static/src/two.js", ("include", "one"), ("include", "two")],
            }"#,
            &["one.js", "two.js"],
            &[],
        );
        let bundles = contents(&[a]);
        assert_eq!(bundles["one"], ["a/one.js", "a/two.js"]);
        assert_eq!(bundles["two"], ["a/two.js", "a/one.js"]);
    }

    #[test]
    fn a_dependent_module_removes_a_file_of_its_depend() {
        let web = addon(
            "web",
            r#""assets": {"b": ["web/static/src/*"]}"#,
            &["a.js", "old.js"],
            &["old.xml"],
        );
        let x = addon(
            "x",
            r#""depends": ["web"], "assets": {"b": [
                ("remove", "web/static/src/old.*"),
                ("replace", "web/static/src/a.js", "x/static/src/a.js"),
            ]}"#,
            &["a.js"],
            &[],
        );
        assert_eq!(contents(&[web, x])["b"], ["x/a.js"]);
    }

    #[test]
    fn keeps_a_file_removed_before_it_is_appended() {
        let a = addon(
            "a",
            r#""assets": {"b": [("remove", "a/static/src/a.js"), "a/static/src/a.js"]}"#,
            &["a.js"],
            &[],
        );
        assert_eq!(contents(&[a])["b"], ["a/a.js"]);
    }

    #[test]
    fn appends_when_the_target_is_not_listed() {
        let a = addon(
            "a",
            r#""assets": {"b": [
                "a/static/src/a.js",
                ("before", "a/static/src/gone.js", "a/static/src/b.js"),
                ("replace", "a/static/src/none*.js", "a/static/src/c.js"),
            ]}"#,
            &["a.js", "b.js", "c.js"],
            &[],
        );
        assert_eq!(contents(&[a])["b"], ["a/a.js", "a/b.js", "a/c.js"]);
    }

    #[test]
    fn skips_an_entry_whose_target_has_no_asset_extension_and_no_file() {
        let mut a = addon(
            "a",
            r#""assets": {"b": [
                "a/static/src/a.js",
                ("after", "a/static/src/widgets/*", "a/static/src/b.js"),
                ("before", "a/static/src/styles/*", "a/static/src/c.js"),
            ]}"#,
            &["a.js", "b.js", "c.js"],
            &[],
        );
        a.static_assets = vec!["a/static/src/styles/s.scss".to_owned()];
        assert_eq!(contents(&[a])["b"], ["a/a.js", "a/c.js"]);
    }

    #[test]
    fn places_at_a_target_that_is_no_glob_and_no_file() {
        let a = addon(
            "a",
            r#""assets": {"b": [
                "a/static/src/lib/vendor",
                "a/static/src/a.js",
                ("before", "a/static/src/lib/vendor", "a/static/src/b.js"),
                ("after", "a/static/src/lib/gone", "a/static/src/c.js"),
            ]}"#,
            &["a.js", "b.js", "c.js"],
            &[],
        );
        assert_eq!(contents(&[a])["b"], ["a/b.js", "a/a.js", "a/c.js"]);
    }

    #[test]
    fn keeps_an_entry_whose_target_module_has_no_static_assets_read() {
        let mut web = addon(
            "web",
            r#""assets": {"b": ["web/static/src/views/*"]}"#,
            &["views/v.js", "views/w.js"],
            &[],
        );
        web.checked = false;
        let x = addon(
            "x",
            r#""depends": ["web"], "assets": {"b": [("after", "web/static/src/views/*", "x/static/src/a.js")]}"#,
            &["a.js"],
            &[],
        );
        assert_eq!(contents(&[web, x])["b"], ["web/views/v.js", "x/a.js", "web/views/w.js"]);
    }

    #[test]
    fn a_remove_after_an_include_leaves_the_included_bundle_whole() {
        let a = addon(
            "a",
            r#""assets": {
                "outer": [("include", "inner"), ("remove", "a/static/src/i.js")],
                "inner": ["a/static/src/i.js", "a/static/src/j.js"],
            }"#,
            &["i.js", "j.js"],
            &[],
        );
        let bundles = contents(&[a]);
        assert_eq!(bundles["outer"], ["a/j.js"]);
        assert_eq!(bundles["inner"], ["a/i.js", "a/j.js"]);
    }
}
