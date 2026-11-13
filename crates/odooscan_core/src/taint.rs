//! Values a user controls, followed through the locals of one function to the calls that must not
//! receive them.

use ruff_python_ast::visitor::{self, Visitor};
use ruff_python_ast::{self as ast, CmpOp, Expr, Operator, Stmt};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::facts::{
    ArgSlot, CallSite, FlowEdge, FlowOrigin, FlowSummary, FlowTarget, Pos, RawDiag, Receiver, RunsAs, Safe, Spanned,
    Step,
};
use crate::method_body::arguments;
use crate::module_state::{self, MUTATORS, dotted};
use crate::python::{Locator, SAME_MODEL, exits, str_lit, tail_name};
use crate::python2;
use crate::series::Series;
use crate::superuser;

/// What the user controls in a tainted value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Value,
    /// The keys of a mapping too, so that it can name any field of a record.
    Keys,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sink {
    Sql,
    Eval,
    Markup,
    SudoWrite,
    Redirect,
    Request,
    Path,
    Command,
    Deserialize,
    Entity,
    Header,
    Model,
    Method,
    Field,
    Domain,
    /// Records a route returns, browsed by an id the user picks under `sudo()`. Only `Flows` finds
    /// it, at a `return`, so no row of `TABLE` or of a configuration names it, and only from an
    /// id the route itself reads from the user, so no summary edge keeps it.
    Record,
    /// Content stored as superuser in an attachment whose values set `public` to `True`. Only
    /// `Flows::publishes` finds it, from the values a create passes, so no row of `TABLE` or of a
    /// configuration names it.
    Attachment,
}

/// The arguments of a call that a sink checks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Arg {
    /// The first positional argument, or without one a keyword of `SINK_KEYWORDS`.
    First,
    /// The positional argument at the index, or without it one of the keywords.
    At(usize, &'static [&'static str]),
    /// Every positional argument.
    Any,
    /// Each of these, as the domain and the fields of `search_read()`.
    Each(&'static [Arg]),
}

/// What the argument a guard reads must be for a call to be the sink.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Test {
    /// A boolean literal, as `local=False` for `request.redirect()`.
    Is(bool),
    /// Anything but a value whose last name is listed, as a YAML loader other than a safe one.
    Unless(&'static [&'static str]),
    /// An lxml parser that resolves entities: `XMLParser(resolve_entities=True)`, and before the
    /// series also the default parser or an `XMLParser` built without the keyword, as lxml before
    /// 5 resolves by default.
    Resolves(Series),
}

/// The argument at a keyword or position that tells whether a call is the sink. Before `since`
/// the callee has no such parameter and always acts as the test asks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Guard {
    keyword: &'static str,
    pos: usize,
    test: Test,
    since: Option<Series>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SinkRow {
    sink: Sink,
    arg: Arg,
    guard: Option<Guard>,
    /// The pattern matches the bare name alone, so that `Image.open` is no `open`.
    whole: bool,
    /// The path names the standard library's module, not the one of that name an import from
    /// `odoo` binds: 17.0 `odoo.tools.pickle` loads only plain data (`odoo/tools/misc.py:1689`).
    stdlib: bool,
    /// The argument in a module on Python 2, where it differs from `arg`.
    python2: Option<Arg>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    Source(Kind),
    Sink(SinkRow),
    Sanitizer,
    /// Returns what its arguments carry, made harmless for the rule of that code.
    Clears(&'static str),
    /// Returns what the positional arguments at these indexes, or these keywords, carry, and
    /// nothing of the others.
    Keeps(&'static [usize], &'static [&'static str]),
    /// Builds a domain's condition from a field, an operator and a value, as `Domain(f, op, v)`.
    Leaf,
}

const fn sink(sink: Sink) -> Role {
    sink_at(sink, Arg::First)
}

const fn sink_at(sink: Sink, arg: Arg) -> Role {
    Role::Sink(SinkRow {
        sink,
        arg,
        guard: None,
        whole: false,
        stdlib: false,
        python2: None,
    })
}

const fn guarded(sink: Sink, arg: Arg, guard: Guard) -> Role {
    Role::Sink(SinkRow {
        sink,
        arg,
        guard: Some(guard),
        whole: false,
        stdlib: false,
        python2: None,
    })
}

const fn stdlib(sink: Sink, arg: Arg) -> Role {
    Role::Sink(SinkRow {
        sink,
        arg,
        guard: None,
        whole: false,
        stdlib: true,
        python2: None,
    })
}

/// The keywords of the URL of a redirect: `url` in the `request.redirect` of 14.0 and before.
const LOCATION: Arg = Arg::At(0, &["location", "url"]);
const URL: Arg = Arg::At(0, &["url"]);
const FILE: Arg = Arg::At(0, &["file", "path_or_file", "filepath_or_fp"]);
/// The built-in `open(name, mode, buffering)` of Python 2, which 9.0 and 10.0 run on, and the
/// `file` of the `io.open()` that `from io import open` binds to the same bare name.
const OPEN_PY2: Arg = Arg::At(0, &["name", "file"]);

/// From 15.0 `request.redirect()` takes `local`, which drops the scheme and the host of the URL
/// unless it is false (`odoo/http.py:303`); before, it redirects to any URL.
const REDIRECT_LOCAL: Guard = Guard {
    keyword: "local",
    pos: 2,
    test: Test::Is(false),
    since: Some(Series { major: 15, minor: 0 }),
};

/// The command of a `subprocess` call, which a shell runs only with `shell=True`, the ninth
/// parameter of `Popen`.
const COMMAND: Arg = Arg::At(0, &["args"]);
const SHELL: Guard = Guard {
    keyword: "shell",
    pos: 8,
    test: Test::Is(true),
    since: None,
};

const DATA: Arg = Arg::At(0, &["data", "file", "stream"]);

/// A YAML loader that builds only plain values.
const SAFE_LOADER: Guard = Guard {
    keyword: "Loader",
    pos: 1,
    test: Test::Unless(&["SafeLoader", "CSafeLoader", "BaseLoader", "CBaseLoader"]),
    since: None,
};

const XML: Arg = Arg::At(0, &["text", "source", "xml"]);

/// From saas~19.1 Odoo needs Python 3.12 (`odoo/release.py:39`) and so lxml 5.2.1
/// (`requirements.txt:21`), whose default parser resolves no external entity; 18.0 pins lxml 4
/// below Python 3.12 (`requirements.txt:38-39`). Odoo's own default parser is set per thread only.
const RESOLVES: Guard = Guard {
    keyword: "parser",
    pos: 1,
    test: Test::Resolves(Series { major: 19, minor: 1 }),
    since: None,
};

const HEADERS: Arg = Arg::At(1, &["headers"]);

/// The domain of a search, the keyword `args` of `search()` and `search_count()` through 15.0
/// (`odoo/models.py:1792`, `:1805`).
const DOMAIN: Arg = Arg::At(0, &["domain", "args"]);
/// The fields `search_read()` and `search_fetch()` read, a list of names rather than a domain.
const FIELD_NAMES: Arg = Arg::At(1, &["fields", "field_names"]);
const SEARCH_READ: Arg = Arg::Each(&[DOMAIN, FIELD_NAMES]);
/// `read_group(domain, fields, groupby)` through 19.0 (18.0 `odoo/models.py:2819`), and
/// `(domain, groupby, aggregates)` for `_read_group` from 17.0 (18.0 `odoo/models.py:1978`),
/// `formatted_read_group` from 19.0 and `read_group` in 20.0 (`odoo/orm/models.py:1932`,
/// `addons/web/models/models.py:1022`).
const READ_GROUP: Arg = Arg::Each(&[
    DOMAIN,
    Arg::At(1, &["fields", "groupby"]),
    Arg::At(2, &["groupby", "aggregates"]),
]);

/// Where user data enters, where it must not arrive, and what makes it harmless. A pattern matches
/// a dotted callee or attribute that ends with its last name and holds its other names in order, so
/// `cr.execute` matches `self.env.cr.execute` and `sudo().write` matches
/// `env["x"].sudo().browse(ids).write`. A source's names stand together, so that
/// `request.httprequest.files.values()` reads the files rather than the `values` source. The
/// parameters of routes and of public model methods are sources too; `python.rs` tells which
/// functions those are.
const TABLE: &[(&str, Role)] = &[
    ("request.params", Role::Source(Kind::Keys)),
    ("request.get_http_params", Role::Source(Kind::Keys)),
    ("request.get_json_data", Role::Source(Kind::Keys)),
    ("request.jsonrequest", Role::Source(Kind::Keys)),
    // Its form from saas~15.4 (16.0 addons/payment_adyen/controllers/main.py:274), which the
    // contiguous match of a source does not read as the row above.
    ("request.dispatcher.jsonrequest", Role::Source(Kind::Keys)),
    ("request.httprequest.args", Role::Source(Kind::Keys)),
    ("request.httprequest.form", Role::Source(Kind::Keys)),
    ("request.httprequest.values", Role::Source(Kind::Keys)),
    ("request.httprequest.files", Role::Source(Kind::Keys)),
    ("request.httprequest.cookies", Role::Source(Kind::Keys)),
    ("request.httprequest.headers", Role::Source(Kind::Keys)),
    ("request.httprequest.get_json", Role::Source(Kind::Keys)),
    ("request.httprequest.data", Role::Source(Kind::Value)),
    ("request.httprequest.get_data", Role::Source(Kind::Value)),
    ("cr.execute", sink(Sink::Sql)),
    ("_cr.execute", sink(Sink::Sql)),
    ("SQL", sink(Sink::Sql)),
    ("safe_eval", sink(Sink::Eval)),
    ("eval", sink(Sink::Eval)),
    ("exec", sink(Sink::Eval)),
    ("Markup", sink(Sink::Markup)),
    ("sudo().create", sink(Sink::SudoWrite)),
    ("sudo().write", sink(Sink::SudoWrite)),
    ("werkzeug.utils.redirect", sink_at(Sink::Redirect, LOCATION)),
    ("werkzeug.redirect", sink_at(Sink::Redirect, LOCATION)),
    ("request.redirect", guarded(Sink::Redirect, LOCATION, REDIRECT_LOCAL)),
    (
        "request.redirect_query",
        guarded(
            Sink::Redirect,
            LOCATION,
            Guard {
                pos: 3,
                since: None,
                ..REDIRECT_LOCAL
            },
        ),
    ),
    ("requests.get", sink_at(Sink::Request, URL)),
    ("requests.post", sink_at(Sink::Request, URL)),
    ("requests.put", sink_at(Sink::Request, URL)),
    ("requests.patch", sink_at(Sink::Request, URL)),
    ("requests.delete", sink_at(Sink::Request, URL)),
    ("requests.head", sink_at(Sink::Request, URL)),
    ("requests.options", sink_at(Sink::Request, URL)),
    ("requests.request", sink_at(Sink::Request, Arg::At(1, &["url"]))),
    ("urlopen", sink_at(Sink::Request, URL)),
    (
        "open",
        Role::Sink(SinkRow {
            sink: Sink::Path,
            arg: FILE,
            guard: None,
            whole: true,
            stdlib: false,
            python2: Some(OPEN_PY2),
        }),
    ),
    ("io.open", sink_at(Sink::Path, FILE)),
    ("send_file", sink_at(Sink::Path, FILE)),
    ("os.path.join", sink_at(Sink::Path, Arg::Any)),
    ("os.system", sink_at(Sink::Command, Arg::At(0, &["command"]))),
    ("os.popen", sink_at(Sink::Command, Arg::At(0, &["cmd"]))),
    ("subprocess.getoutput", sink_at(Sink::Command, Arg::At(0, &["cmd"]))),
    (
        "subprocess.getstatusoutput",
        sink_at(Sink::Command, Arg::At(0, &["cmd"])),
    ),
    ("subprocess.run", guarded(Sink::Command, COMMAND, SHELL)),
    ("subprocess.call", guarded(Sink::Command, COMMAND, SHELL)),
    ("subprocess.check_call", guarded(Sink::Command, COMMAND, SHELL)),
    ("subprocess.check_output", guarded(Sink::Command, COMMAND, SHELL)),
    ("subprocess.Popen", guarded(Sink::Command, COMMAND, SHELL)),
    ("pickle.loads", stdlib(Sink::Deserialize, DATA)),
    ("pickle.load", stdlib(Sink::Deserialize, DATA)),
    ("marshal.loads", sink_at(Sink::Deserialize, DATA)),
    ("marshal.load", sink_at(Sink::Deserialize, DATA)),
    ("yaml.load", guarded(Sink::Deserialize, DATA, SAFE_LOADER)),
    ("yaml.load_all", guarded(Sink::Deserialize, DATA, SAFE_LOADER)),
    ("yaml.unsafe_load", sink_at(Sink::Deserialize, DATA)),
    ("yaml.unsafe_load_all", sink_at(Sink::Deserialize, DATA)),
    ("etree.fromstring", guarded(Sink::Entity, XML, RESOLVES)),
    ("etree.XML", guarded(Sink::Entity, XML, RESOLVES)),
    ("etree.parse", guarded(Sink::Entity, XML, RESOLVES)),
    ("objectify.fromstring", guarded(Sink::Entity, XML, RESOLVES)),
    ("make_response", sink_at(Sink::Header, HEADERS)),
    ("make_json_response", sink_at(Sink::Header, HEADERS)),
    ("Response", sink_at(Sink::Header, Arg::At(2, &["headers"]))),
    ("headers.add", sink_at(Sink::Header, Arg::Any)),
    ("headers.set", sink_at(Sink::Header, Arg::Any)),
    ("sudo().read", sink_at(Sink::Field, Arg::At(0, &["fields"]))),
    ("sudo().mapped", sink_at(Sink::Field, Arg::At(0, &["func"]))),
    ("sudo().search", sink_at(Sink::Domain, DOMAIN)),
    ("sudo().search_count", sink_at(Sink::Domain, DOMAIN)),
    ("sudo().search_read", sink_at(Sink::Domain, SEARCH_READ)),
    ("sudo().search_fetch", sink_at(Sink::Domain, SEARCH_READ)),
    ("sudo().read_group", sink_at(Sink::Domain, READ_GROUP)),
    ("sudo()._read_group", sink_at(Sink::Domain, READ_GROUP)),
    ("sudo().formatted_read_group", sink_at(Sink::Domain, READ_GROUP)),
    ("Domain", Role::Leaf),
    ("int", Role::Sanitizer),
    ("float", Role::Sanitizer),
    ("bool", Role::Sanitizer),
    ("len", Role::Sanitizer),
    ("SQL.identifier", Role::Sanitizer),
    ("escape", Role::Sanitizer),
    ("html_escape", Role::Sanitizer),
    ("html_sanitize", Role::Sanitizer),
    ("plaintext2html", Role::Sanitizer),
    // QWeb escapes the values it renders.
    ("_render", Role::Sanitizer),
    ("_render_template", Role::Sanitizer),
    ("urlencode", Role::Sanitizer),
    ("url_encode", Role::Sanitizer),
    ("secure_filename", Role::Sanitizer),
    // Its file lies under the addons paths, whatever name it is given (18.0
    // odoo/tools/misc.py:187-260).
    ("file_open", Role::Sanitizer),
    // An addon's manifest from the addons paths, for a name matching `^\w{1,256}$`, or `None` (20.0
    // odoo/modules/module.py:56, :338-351).
    ("Manifest.for_addon", Role::Sanitizer),
    // Quotes a file name for the header, so that it ends no parameter (18.0 odoo/http.py:328-331).
    ("content_disposition", Role::Sanitizer),
    // An attribute's value, or the default, and not its name.
    ("getattr", Role::Keeps(&[0, 2], &[])),
    // The query parameters go URL-encoded into the query of the URL (portal/controllers/portal.py).
    ("_build_url_w_params", Role::Keeps(&[0], &["url_string"])),
    // A name without its directories, or a stream of the data, opens no other file.
    ("basename", Role::Clears("OD707")),
    ("BytesIO", Role::Clears("OD707")),
    ("StringIO", Role::Clears("OD707")),
    // Each value they quote is one shell word, whatever it holds.
    ("shlex.quote", Role::Clears("OD708")),
    ("shlex.join", Role::Clears("OD708")),
];

/// The rule that reports each sink.
const SINKS: &[(Sink, &str)] = &[
    (Sink::Sql, "OD701"),
    (Sink::Eval, "OD702"),
    (Sink::Markup, "OD703"),
    (Sink::SudoWrite, "OD704"),
    (Sink::Redirect, "OD705"),
    (Sink::Request, "OD706"),
    (Sink::Path, "OD707"),
    (Sink::Command, "OD708"),
    (Sink::Deserialize, "OD709"),
    (Sink::Entity, "OD710"),
    (Sink::Header, "OD711"),
    (Sink::Model, "OD712"),
    (Sink::Method, "OD713"),
    (Sink::Field, "OD714"),
    (Sink::Domain, "OD715"),
    (Sink::Record, "OD716"),
    (Sink::Attachment, "OD717"),
];

/// Keywords that carry the argument a sink checks when it is not passed by position.
const SINK_KEYWORDS: &[&str] = &["query", "code", "expr", "vals", "vals_list"];

/// The built-in row a dotted path matches, with its pattern.
fn built_in(path: &[String]) -> Option<(&'static str, Role)> {
    TABLE.iter().copied().find(|(pattern, role)| {
        let whole = matches!(role, Role::Sink(row) if row.whole);
        let source = matches!(role, Role::Source(_));
        let fits = if source {
            ends_with(pattern, path)
        } else {
            matches_path(pattern, path)
        };
        fits && (!whole || path.len() == 1)
    })
}

/// Whether `path` ends with the names of `pattern`, none between them.
fn ends_with(pattern: &str, path: &[String]) -> bool {
    let names: Vec<&str> = pattern.split('.').collect();
    let start = path.len().checked_sub(names.len());
    start.is_some_and(|start| path[start..].iter().zip(names).all(|(p, n)| p == n))
}

/// The rows a configuration adds to `TABLE`, matched by the same dotted patterns. They match
/// first, so that a team's `sandbox.safe_eval` sanitizer wins over the built-in `safe_eval` sink.
/// A configured source is read as a mapping, whose keys the user picks too.
#[derive(Clone, Debug, Default)]
pub struct TaintTable {
    rows: Vec<(String, Role)>,
}

impl TaintTable {
    pub fn source(&mut self, pattern: &str) -> Result<(), String> {
        self.add(pattern, Role::Source(Kind::Keys))
    }

    pub fn sanitizer(&mut self, pattern: &str) -> Result<(), String> {
        self.add(pattern, Role::Sanitizer)
    }

    /// A sink reported as `rule`, the code or name of a rule of `SINKS` that a call reports.
    pub fn sink(&mut self, rule: &str, pattern: &str) -> Result<(), String> {
        let code = crate::rules::find(rule).map(|r| r.code);
        let mut called = SINKS
            .iter()
            .filter(|(s, _)| !matches!(s, Sink::Record | Sink::Attachment));
        let Some((kind, _)) = called.clone().find(|(_, c)| Some(*c) == code) else {
            let (first, last) = (SINKS[0].1, called.next_back().map_or("", |(_, c)| *c));
            return Err(format!(
                "'{rule}' is not a taint rule a call reports: name {first} to {last} by code or name"
            ));
        };
        self.add(pattern, sink(*kind))
    }

    fn add(&mut self, pattern: &str, role: Role) -> Result<(), String> {
        if !valid_pattern(pattern) {
            return Err(format!(
                "'{pattern}' is not a taint pattern: write dotted names such as cr.execute, where [] stands for an item. A name but the last may end in () for a call, and a last [] matches a call of an item, never an item read"
            ));
        }
        self.rows.push((pattern.to_owned(), role));
        Ok(())
    }

    /// The row a dotted path matches, with its pattern: a configured one before a built-in one.
    fn lookup(&self, path: &[String]) -> Option<(&str, Role)> {
        self.rows
            .iter()
            .find(|(pattern, _)| matches_path(pattern, path))
            .map(|(pattern, role)| (pattern.as_str(), *role))
            .or_else(|| built_in(path))
    }
}

/// Whether each segment of `pattern` is a name, a name and `()`, or `[]`, the last no call, as
/// `Flows::path` writes a path and `matches_path` compares one. A last `[]` stays valid: an item
/// read never looks up its path, so it matches `handlers[k](x)` but never `handlers[k]`.
fn valid_pattern(pattern: &str) -> bool {
    let identifier = |s: &str| {
        let mut chars = s.chars();
        chars.next().is_some_and(|c| c == '_' || c.is_alphabetic()) && chars.all(|c| c == '_' || c.is_alphanumeric())
    };
    let segments: Vec<&str> = pattern.split('.').collect();
    let last = segments.len() - 1;
    segments
        .iter()
        .enumerate()
        .all(|(i, segment)| match segment.strip_suffix("()") {
            Some(name) => i != last && identifier(name),
            None => *segment == "[]" || identifier(segment),
        })
}

fn matches_path(pattern: &str, path: &[String]) -> bool {
    let mut names = pattern.split('.').rev();
    let Some((last, rest)) = path.split_last() else {
        return false;
    };
    if names.next() != Some(last.as_str()) {
        return false;
    }
    let mut rest = rest.iter().rev();
    names.all(|name| rest.any(|p| p == name))
}

/// Which parameters of a function a user sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Params<'a> {
    None,
    /// A controller method behind `@route`: its URL and form parameters, but for those named here,
    /// which a converter of the URL fills with records.
    Route(&'a [&'a str]),
    /// A public model method, which any user with access to the model can call over RPC.
    Rpc,
}

#[derive(Clone, Debug)]
struct Taint {
    kind: Kind,
    origin: FlowOrigin,
    /// Where the value entered, then each local it was assigned to.
    steps: Vec<Spanned<String>>,
    safe: Safe,
    /// The value is the `name` of a parameter's own value, harmless when a caller passes a file
    /// picked by its field.
    named: bool,
    /// The value is a parameter's own, copied but not derived, so a caller's file when it is one.
    own: bool,
    upload: Option<Upload>,
    /// Each value of a parameter or a call joined into this one, as it is alone, so that each
    /// keeps its own edge: the primary's among them unless it is a source. Empty when the value
    /// has one origin.
    origins: Vec<Taint>,
}

/// What a value holds of the files a request uploads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Upload {
    /// Every file, by the field of the form that sent it, which the user names.
    Mapping,
    /// The files of a field the code names, whose `name` werkzeug sets to that field (3.0.1
    /// `formparser.py:392-400`).
    Picked,
}

impl Taint {
    /// The value derived from this one, of `kind`.
    fn kind(self, kind: Kind) -> Self {
        self.each(|t| {
            t.kind = kind;
            t.own = false;
        })
    }

    fn safe(self, safe: Safe) -> Self {
        self.each(|t| t.safe = t.safe.union(safe))
    }

    /// Changes the value and each origin joined into it alike.
    fn each(mut self, f: impl Fn(&mut Taint)) -> Self {
        self.each_mut(f);
        self
    }

    fn each_mut(&mut self, f: impl Fn(&mut Taint)) {
        f(self);
        self.origins.iter_mut().for_each(f);
    }

    /// Takes the values of a parameter or a call this one holds, each as it is alone.
    fn take_origins(&mut self) -> Vec<Taint> {
        if !self.origins.is_empty() {
            std::mem::take(&mut self.origins)
        } else if self.origin == FlowOrigin::Source {
            vec![]
        } else {
            vec![self.clone()]
        }
    }

    /// Adds one step to the value and to each origin.
    fn step(&mut self, step: &Spanned<String>) {
        self.each_mut(|t| t.steps.push(step.clone()));
    }

    /// The values that each keep an edge: the source when it is the primary, then each origin as
    /// it is alone. The primary's comes first: of the findings several origins make at one sink,
    /// the solver keeps the first unless a later one passes through fewer methods, and `primary`
    /// picks the value a sink may report.
    fn split(mut self) -> Vec<Taint> {
        let mut origins = std::mem::take(&mut self.origins);
        if origins.is_empty() {
            return vec![self];
        }
        if let Some(i) = origins.iter().position(|o| o.origin == self.origin) {
            let primary = origins.remove(i);
            origins.insert(0, primary);
        }
        if self.origin == FlowOrigin::Source {
            origins.insert(0, self);
        }
        origins
    }
}

fn join(a: Option<Taint>, b: Option<Taint>) -> Option<Taint> {
    match (a, b) {
        (Some(a), Some(b)) => Some(joined(a, b)),
        (a, b) => a.or(b),
    }
}

/// The value two values join into: the primary `primary` picks, which a finding in the function
/// reads, holding the origins of either, which each keep their own edge.
fn joined(mut a: Taint, mut b: Taint) -> Taint {
    let origins = union_origins(a.take_origins(), b.take_origins());
    let mut t = primary(a, b);
    // One origin alone is the primary itself.
    let alone = t.origin != FlowOrigin::Source && origins.len() == 1;
    t.origins = if alone { vec![] } else { origins };
    t
}

/// Adds the origins of `other` to `mine`, joining two of the same origin into one.
fn union_origins(mut mine: Vec<Taint>, other: Vec<Taint>) -> Vec<Taint> {
    for o in other {
        match mine.iter().position(|m| m.origin == o.origin) {
            Some(i) => {
                let m = mine.remove(i);
                mine.insert(i, primary(m, o));
            }
            None => mine.push(o),
        }
    }
    mine
}

/// Of two joined values, the one the user sets on entry wins over one a caller or callee may taint,
/// so that a finding in the function is never traded for a summary edge. Among those, a part of a
/// domain the user may pick wins over a value a condition holds, then a value harmless for fewer
/// rules, both with the keys of either, so that a finding names the value a sink may report; then
/// keys win so that OD704 still sees them. A source joined to an RPC argument stays the source,
/// with the keys of either: a Python caller sets only the argument, so its edge would drop the
/// source. The result is harmless only where both are, but for OD715 when what a call returns is
/// dropped: its domain is as often one the code built.
fn primary(a: Taint, b: Taint) -> Taint {
    let kind = a.kind.max(b.kind);
    let (second, widen) = pick(&a, &b);
    let (keep, drop) = if second { (b, a) } else { (a, b) };
    let keep = if widen { keep.kind(kind) } else { keep };
    let returned = matches!(drop.origin, FlowOrigin::Return(_)) && drop.origin != keep.origin;
    let dropped = if returned {
        drop.safe.union(Safe::of(&["OD715"]))
    } else {
        drop.safe
    };
    Taint {
        safe: keep.safe.intersect(dropped),
        named: keep.named && drop.named,
        own: keep.own && drop.own,
        upload: keep.upload.filter(|u| drop.upload == Some(*u)),
        ..keep
    }
}

/// Whether a join keeps the second of two values, and whether it gives it the keys of either.
fn pick(a: &Taint, b: &Taint) -> (bool, bool) {
    let rank = |t: &Taint| (t.origin.is_user(), t.kind);
    match (a.origin, b.origin) {
        (FlowOrigin::Source, FlowOrigin::Rpc(_)) => (false, true),
        (FlowOrigin::Rpc(_), FlowOrigin::Source) => (true, true),
        _ => match pick_peer(a, b) {
            Some(second) => (second, true),
            None => (rank(b) > rank(a), false),
        },
    }
}

/// Whether a join keeps the second of two values the user sets alike: the one a condition does
/// not hold, else the one harmless for fewer rules. `None` when only one of them comes from the
/// user, or when both are held alike and neither's rules contain the other's.
fn pick_peer(a: &Taint, b: &Taint) -> Option<bool> {
    let held = |t: &Taint| t.safe.has("OD715");
    let fewer = |x: &Taint, y: &Taint| x.safe != y.safe && x.safe.intersect(y.safe) == x.safe;
    if a.origin.is_user() != b.origin.is_user() {
        return None;
    }
    if held(a) != held(b) {
        return Some(held(a));
    }
    (fewer(a, b) || fewer(b, a)).then(|| fewer(b, a))
}

/// The codes a string's start keeps harmless when the code, not the user, picks it: a redirect or
/// a request then goes to the host the code chose.
const FIXED: &[&str] = &["OD705", "OD706"];

/// Whether a URL that starts with `s` has the host the code chose, whatever follows: `s` puts what
/// follows in the query or the fragment, is a path on the same host, or names its host.
fn fixes_start(s: &str) -> bool {
    let path = s
        .strip_prefix('/')
        .and_then(|rest| rest.chars().next())
        .is_some_and(|c| c != '/' && c != '\\');
    let host = s.split_once("://").is_some_and(|(scheme, rest)| {
        let scheme_ok = scheme.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
            && scheme.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c));
        scheme_ok
            && rest
                .split_once('/')
                .is_some_and(|(host, _)| !host.is_empty() && !host.contains('\\'))
    });
    s.contains(['?', '#']) || path || host
}

/// What a string that starts with `expr`, of taint `t`, keeps harmless for its start.
fn start_of(expr: &Expr, t: Option<&Taint>) -> Safe {
    match constant(expr) {
        Some(s) => literal_start(&s),
        None => start_of_value(t),
    }
}

fn literal_start(s: &str) -> Safe {
    if fixes_start(s) {
        Safe::of(FIXED)
    } else {
        Safe::default()
    }
}

/// A value the user does not control, or one that starts as the code picked.
fn start_of_value(t: Option<&Taint>) -> Safe {
    t.map_or(Safe::of(FIXED), |t| t.safe.intersect(Safe::of(FIXED)))
}

/// The text of a string the code writes out whole: a literal, an f-string without fields, or a
/// concatenation of them.
fn constant(expr: &Expr) -> Option<String> {
    match expr {
        Expr::StringLiteral(s) => Some(s.value.to_str().to_owned()),
        Expr::FString(f) => fstring_text(f),
        Expr::BinOp(b) if b.op == Operator::Add => Some(constant(&b.left)? + &constant(&b.right)?),
        _ => None,
    }
}

fn fstring_text(f: &ast::ExprFString) -> Option<String> {
    let mut text = String::new();
    for part in f.value.iter() {
        let elements = match part {
            ast::FStringPartRef::Literal(l) => {
                text.push_str(&l.value);
                continue;
            }
            ast::FStringPartRef::FString(f) => &f.elements,
        };
        for e in elements {
            let ast::InterpolatedStringElement::Literal(l) = e else {
                return None;
            };
            text.push_str(&l.value);
        }
    }
    Some(text)
}

/// Where `left <op> right` starts, `first` being the first value `%` puts in a template.
fn bin_start(b: &ast::ExprBinOp, left: Option<&Taint>, first: Option<(&Expr, Option<Taint>)>) -> Safe {
    match b.op {
        Operator::Add => start_of(&b.left, left),
        Operator::Mod => constant(&b.left).map_or_else(Safe::default, |template| {
            match template_prefix(&template, '%') {
                (prefix, _) if !prefix.is_empty() => literal_start(&prefix),
                // A mapping's values have no first one.
                (_, rest) if rest.starts_with('(') => Safe::default(),
                _ => first.map_or_else(Safe::default, |(e, t)| start_of(e, t.as_ref())),
            }
        }),
        _ => Safe::default(),
    }
}

/// What a name built around text the code writes keeps harmless: one with a letter or a digit of
/// its own names a method of a family the code picks, as `'_get_%s_view' % t` does, where `'_' + t`
/// names any private method.
fn affixed(text: &str) -> Safe {
    if text.chars().any(char::is_alphanumeric) {
        Safe::of(&["OD713"])
    } else {
        Safe::default()
    }
}

/// The text `+` or a literal template's `%` writes around the values it takes.
fn bin_affix(b: &ast::ExprBinOp) -> Safe {
    let text = match b.op {
        Operator::Add => [&*b.left, &*b.right].into_iter().filter_map(constant).collect(),
        Operator::Mod => constant(&b.left).map(|t| literal_text(&t, '%')).unwrap_or_default(),
        _ => String::new(),
    };
    affixed(&text)
}

fn format_affix(call: &ast::ExprCall) -> Safe {
    let Expr::Attribute(a) = &*call.func else {
        return Safe::default();
    };
    let template = constant(&a.value).filter(|_| a.attr.as_str() == "format");
    template.map_or_else(Safe::default, |t| affixed(&literal_text(&t, '{')))
}

/// The text of a template outside its fields, which `open` starts.
fn literal_text(template: &str, open: char) -> String {
    let mut text = String::new();
    let mut rest = template;
    while !rest.is_empty() {
        let (prefix, field) = template_prefix(rest, open);
        text.push_str(&prefix);
        rest = past_field(field, open);
    }
    text
}

/// What follows a field whose `open` is gone: `%(name)-5.2f` or `{0!r:>10}`.
fn past_field(field: &str, open: char) -> &str {
    if open == '{' {
        return field.split_once('}').map_or("", |(_, rest)| rest);
    }
    let field = match field.strip_prefix('(') {
        Some(named) => named.split_once(')').map_or("", |(_, rest)| rest),
        None => field,
    };
    let field = field.trim_start_matches(|c: char| c.is_ascii_digit() || "#0- +*.hlL".contains(c));
    let mut chars = field.chars();
    chars.next();
    chars.as_str()
}

/// The text of a template before its first field, which `open` starts, and what follows the
/// field's `open`. A doubled `open` is the character itself.
fn template_prefix(template: &str, open: char) -> (String, &str) {
    let mut prefix = String::new();
    let mut chars = template.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c != open {
            prefix.push(c);
        } else if chars.peek().is_some_and(|(_, next)| *next == open) {
            prefix.push(c);
            chars.next();
        } else {
            return (prefix, &template[i + c.len_utf8()..]);
        }
    }
    (prefix, "")
}

fn values(t: Option<Taint>) -> Option<Taint> {
    t.map(|t| t.kind(Kind::Value))
}

/// The source of the files a request uploads.
const FILES: &str = "request.httprequest.files";

/// What the attribute `name` of a value of taint `t` carries. The `name` of a file picked by its
/// field is that field, which the code wrote.
fn attribute(t: Option<Taint>, name: &str) -> Option<Taint> {
    let t = t.filter(|t| name != "name" || t.upload != Some(Upload::Picked))?;
    let t = t.each(|t| {
        t.named = name == "name" && t.own;
        t.upload = None;
    });
    Some(t.kind(Kind::Value))
}

/// What an item of a value of taint `t` holds, `key` being the item's literal key: a file of the
/// uploads by a field the code names is picked, and so are those of a list of picked files.
fn item_of(t: Option<Taint>, key: Option<&str>) -> Option<Taint> {
    t.map(|t| {
        let t = t.each(|t| {
            if t.upload == Some(Upload::Mapping) {
                t.upload = key.map(|_| Upload::Picked);
            }
        });
        t.kind(Kind::Value)
    })
}

#[derive(Clone, Default)]
struct State {
    vars: FxHashMap<String, Taint>,
    /// Locals bound to a recordset under `sudo()`, with the segments of its path that make it so.
    sudo: FxHashMap<String, Vec<String>>,
    /// Locals bound to records rather than to what a method of them returns.
    records: FxHashSet<String>,
    /// Locals bound to a model, as `Orders = env["sale.order"]` does.
    models: FxHashMap<String, Receiver>,
    /// Locals bound to an lxml `XMLParser()`.
    parsers: FxHashMap<String, Entities>,
    /// Locals bound to a dotted name, by its last name, as `Loader = yaml.SafeLoader`.
    names: FxHashMap<String, String>,
    /// Locals bound to the records of a model the user picks, as `env[t]` does, with the taint of
    /// the model's name.
    picked: FxHashMap<String, Taint>,
    /// Locals that hold records browsed by an id the user picks, or a value holding them.
    browsed: FxHashMap<String, Browsed>,
    /// Locals a check vouches for, whose records the route may show: an id naming one of them is
    /// not one the user picks for OD716 when each local in it that holds a value is checked too.
    checked: FxHashSet<String>,
    /// Locals bound to `int()` or `float()` of a value the user sets, which still pick records.
    ids: FxHashMap<String, Taint>,
    /// Locals bound to values a create of `ir.attachment` may take.
    vals: FxHashMap<String, Vals>,
    /// Locals bound to a path that an `os.path.join()` of `Flows::held` built.
    paths: FxHashMap<String, Sites>,
    /// Locals and attributes of `self` bound to an outgoing client of `CLIENTS`.
    clients: FxHashSet<String>,
    /// Locals bound to a header name or to a list or mapping of headers, by the header
    /// `Flows::header_name` reads in it.
    headers: FxHashMap<String, String>,
    /// Locals bound to a value `Flows::chosen` or `Flows::fixed` reads as the code's, by whether
    /// it is a collection, and that took no value since.
    chosen: FxHashMap<String, bool>,
}

/// The `os.path.join()` calls a path may come from, and whether a normalizer resolved its `..` on
/// every path, so that a test of its start holds for what it opens.
#[derive(Clone, Debug)]
struct Sites {
    at: Vec<Pos>,
    normalized: bool,
    /// A check of its start holds here on every path, so a read uses no join unchecked.
    confined: bool,
}

fn join_sites(a: Option<Sites>, b: Option<Sites>) -> Option<Sites> {
    match (a, b) {
        (Some(mut a), Some(b)) => {
            for pos in b.at {
                if !a.at.contains(&pos) {
                    a.at.push(pos);
                }
            }
            a.normalized &= b.normalized;
            a.confined &= b.confined;
            Some(a)
        }
        (a, b) => a.or(b),
    }
}

/// The normalizers that resolve the `..` of a path.
const NORMALIZERS: &[&str] = &["os.path.normpath", "os.path.realpath", "os.path.abspath"];

/// What values for `ir.attachment` set: `public` to a literal `True`, and the taint of the content
/// they store.
#[derive(Clone, Debug, Default)]
struct Vals {
    public: bool,
    /// Every path sets `public`, so that these values replace the `public` of those they are set
    /// over.
    sets_public: bool,
    /// Each key of the content as last set, a clean value too, so that it replaces.
    keys: Vec<Key>,
    /// The taint of what may set any key of the content: a mapping spread in whose keys are
    /// unknown, or the user's own mapping an item set or `|=` adds values to.
    any: Option<Taint>,
}

#[derive(Clone, Debug)]
struct Key {
    name: String,
    taint: Option<Taint>,
    /// Every path sets it, so that it replaces the key of values it is set over.
    sure: bool,
}

impl Vals {
    /// The values of either of two paths. A key one path leaves unset may keep what it held.
    fn and(mut self, other: Vals) -> Vals {
        for key in &mut self.keys {
            key.sure &= other.keys.iter().any(|k| k.name == key.name);
        }
        for key in other.keys {
            match self.keys.iter_mut().find(|k| k.name == key.name) {
                Some(mine) => {
                    mine.taint = join(mine.taint.take(), key.taint);
                    mine.sure &= key.sure;
                }
                None => self.keys.push(Key { sure: false, ..key }),
            }
        }
        self.public |= other.public;
        self.sets_public &= other.sets_public;
        self.any = join(self.any, other.any);
        self
    }

    /// These values with `later` set over them, as `{**self, **later}`: a key `later` sets on every
    /// path replaces this one's, and one it may leave unset joins it.
    fn over(mut self, later: Vals) -> Vals {
        for key in later.keys {
            match self.keys.iter_mut().find(|k| k.name == key.name) {
                Some(mine) if !key.sure => mine.taint = join(mine.taint.take(), key.taint),
                _ => self.set(key.name, key.taint, key.sure),
            }
        }
        if later.sets_public {
            self.public = later.public;
            self.sets_public = true;
        } else {
            self.public |= later.public;
        }
        self.any = join(self.any, later.any);
        self
    }

    fn set(&mut self, name: String, taint: Option<Taint>, sure: bool) {
        match self.keys.iter_mut().find(|k| k.name == name) {
            Some(mine) => *mine = Key { name, taint, sure },
            None => self.keys.push(Key { name, taint, sure }),
        }
    }

    fn set_public(&mut self, public: bool) {
        self.public = public;
        self.sets_public = true;
    }

    fn content(&self) -> Option<Taint> {
        self.keys
            .iter()
            .fold(self.any.clone(), |acc, k| join(acc, k.taint.clone()))
    }

    fn step(&mut self, step: &Spanned<String>) {
        let keys = self.keys.iter_mut().filter_map(|k| k.taint.as_mut());
        for t in keys.chain(self.any.as_mut()) {
            t.step(step);
        }
    }
}

/// The values of a local on either of two paths. A path where it holds none may leave the keys
/// of the other unset, so they replace nothing.
fn join_vals(a: Option<Vals>, b: Option<Vals>) -> Option<Vals> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.and(b)),
        (Some(v), None) | (None, Some(v)) => Some(v.and(Vals::default())),
        (None, None) => None,
    }
}

/// From saas~19.3 `ir.attachment` has no `datas`: 20.0 reads the content from `raw` and `db_datas`
/// only (`odoo/addons/base/models/ir_attachment.py:540-542`).
const NO_DATAS: Series = Series { major: 19, minor: 3 };

/// The methods of `ir.attachment` that create attachments, the argument of their values, and the
/// argument of a file whose content they store too.
const CREATES: &[(&str, Arg, Option<Arg>)] = &[
    ("create", Arg::At(0, &["vals_list", "vals"]), None),
    (
        "_upload_file",
        Arg::At(1, &["create_vals"]),
        Some(Arg::At(0, &["file"])),
    ),
];

/// Records browsed by an id the user picks.
#[derive(Clone, Debug)]
struct Browsed {
    id: Taint,
    /// Whether they are read as superuser.
    sudo: bool,
    /// Whether they belong to uid 1, which `sudo(False)` leaves in superuser mode.
    root: bool,
    /// The locals of the id and those bound to the records or to a value read from them, so that
    /// a check of any of them vouches for the records.
    names: Vec<String>,
}

fn join_browsed(a: Option<Browsed>, b: Option<Browsed>) -> Option<Browsed> {
    match (a, b) {
        (Some(a), Some(b)) => {
            let mut names = a.names;
            for n in b.names {
                if !names.contains(&n) {
                    names.push(n);
                }
            }
            Some(Browsed {
                id: joined(a.id, b.id),
                sudo: a.sudo || b.sudo,
                root: a.root || b.root,
                names,
            })
        }
        (a, b) => a.or(b),
    }
}

impl State {
    fn merge(&mut self, other: State) {
        for (name, t) in other.vars {
            let mine = self.vars.remove(&name);
            if let Some(t) = join(mine, Some(t)) {
                self.vars.insert(name, t);
            }
        }
        // The path of uid 1 wins, as it alone stays superuser after a later `sudo(False)`.
        for (name, path) in other.sudo {
            let root = |p: &Vec<String>| p.iter().any(|s| s == UID_1);
            if !self.sudo.get(&name).is_some_and(|mine| root(mine) && !root(&path)) {
                self.sudo.insert(name, path);
            }
        }
        self.records.extend(other.records);
        self.models.extend(other.models);
        // A parser that may resolve entities on either path may on the merged one.
        for (name, e) in other.parsers {
            let e = self.parsers.get(&name).map_or(e, |mine| e.max(*mine));
            self.parsers.insert(name, e);
        }
        // A name is known only where both paths agree on it.
        self.names.retain(|name, mine| other.names.get(name) == Some(mine));
        self.headers.retain(|name, mine| other.headers.get(name) == Some(mine));
        self.chosen.retain(|name, mine| other.chosen.get(name) == Some(mine));
        join_into(&mut self.picked, other.picked, join);
        join_into(&mut self.browsed, other.browsed, join_browsed);
        join_into(&mut self.ids, other.ids, join);
        for (name, vals) in &mut self.vals {
            if !other.vals.contains_key(name) {
                *vals = join_vals(Some(std::mem::take(vals)), None).unwrap_or_default();
            }
        }
        join_into(&mut self.vals, other.vals, join_vals);
        join_into(&mut self.paths, other.paths, join_sites);
        // A check on one path only still vouches for the records: as at 18.0
        // payment/controllers/portal.py:76-78, the path without it browses no id.
        self.checked.extend(other.checked);
        // A client suppresses a sink, so it holds only where every path bound one.
        self.clients.retain(|key| other.clients.contains(key));
    }
}

/// The names a file binds at import that the flows of its functions read.
pub struct FileNames<'a> {
    /// Those its imports from `odoo` bind.
    pub odoo: &'a [&'a str],
    /// Those bound to a collection of literals, which `module_state::collections` reads.
    pub collections: &'a [&'a str],
    /// Those bound to `odoo.osv.expression.AND` or `OR`, whose list holds domains.
    pub domain_joins: &'a [&'a str],
}

/// The local names of `odoo_names`, as `module_state::odoo_names` lists them, that bind
/// `odoo.osv.expression.AND` or `OR` (18.0 `odoo/osv/expression.py:303`, `:308`).
pub fn domain_joins<'a>(odoo_names: &[(&'a str, String)]) -> Vec<&'a str> {
    let joins = ["odoo.osv.expression.AND", "odoo.osv.expression.OR"];
    odoo_names
        .iter()
        .filter(|(_, path)| joins.contains(&path.as_str()))
        .map(|(local, _)| *local)
        .collect()
}

/// The flows of one function: findings for those that start in it, and its summary when data
/// may cross into or out of the methods it calls. `owner` is what `self` is, and `series` that of
/// the module, an unknown one read as the newest.
pub fn function(
    func: &ast::StmtFunctionDef,
    params: Params,
    owner: Option<Receiver>,
    series: Option<Series>,
    names: &FileNames,
    loc: &Locator,
    table: &TaintTable,
) -> (Vec<RawDiag>, Option<FlowSummary>) {
    let mut flows = Flows {
        loc,
        table,
        route: matches!(params, Params::Route(_)),
        series,
        odoo: names.odoo,
        collections: names.collections,
        domain_joins: names.domain_joins,
        state: State::default(),
        locals: FxHashSet::default(),
        owner,
        calls: vec![],
        edges: vec![],
        diags: vec![],
        held_join: None,
        held: FxHashMap::default(),
        quiet: false,
        testing: false,
    };
    let names = flows.params(&func.parameters, params);
    flows.body(&func.body);
    flows.report_held();
    let mut seen = FxHashSet::default();
    flows.diags.retain(|d| seen.insert((d.code, d.pos)));
    let summary = (!flows.edges.is_empty()).then(|| FlowSummary {
        owner: flows.owner,
        method: func.name.to_string(),
        params: names,
        calls: flows.calls,
        edges: flows.edges,
    });
    (flows.diags, summary)
}

struct Flows<'a> {
    loc: &'a Locator<'a>,
    table: &'a TaintTable,
    /// Whether the function is a route, whose return value the browser receives.
    route: bool,
    series: Option<Series>,
    odoo: &'a [&'a str],
    collections: &'a [&'a str],
    domain_joins: &'a [&'a str],
    state: State,
    /// Every name the function binds, so that `x.f()` on another name is a module's function.
    locals: FxHashSet<String>,
    owner: Option<Receiver>,
    calls: Vec<CallSite>,
    edges: Vec<FlowEdge>,
    diags: Vec<RawDiag>,
    /// The `os.path.join()` the assignment being read binds to one local, held rather than reported.
    held_join: Option<Pos>,
    /// The joins held until a read of their local uses them unchecked.
    held: FxHashMap<Pos, Held>,
    /// The expression being read copies a held path or tests its start, so it uses no join.
    quiet: bool,
    /// The expression being read is the test of an `if` or an `assert`, whose boolean checks
    /// vouch only where they pass, as `Flows::tested` reads them.
    testing: bool,
}

/// A join `Flows::reach` holds.
struct Held {
    taint: Taint,
    callee: String,
    /// A check of its local's start confined it on some path, so it is reported only when read
    /// where none holds.
    checked: bool,
}

impl Flows<'_> {
    /// Taints each parameter but `self`: as a source when the user sets it, otherwise by its
    /// position, for the callers' flows. Returns their names.
    fn params(&mut self, params: &ast::Parameters, source: Params) -> Vec<String> {
        let positional = params.posonlyargs.iter().chain(&params.args).chain(&params.kwonlyargs);
        let named = positional.map(|p| (&p.parameter, false));
        let rest = params.vararg.iter().map(|p| (&**p, false));
        let kwarg = params.kwarg.iter().map(|p| (&**p, true));
        let mut names = vec![];
        for (param, mapping) in named.chain(rest).chain(kwarg) {
            let name = param.name.as_str();
            self.locals.insert(name.to_owned());
            if matches!(name, "self" | "cls") {
                continue;
            }
            let (label, kind, origin) = match source {
                Params::None => ("parameter", Kind::Keys, FlowOrigin::Param(names.len())),
                Params::Route(records) if records.contains(&name) => {
                    names.push(name.to_owned());
                    continue;
                }
                Params::Route(_) if mapping => ("route parameter", Kind::Keys, FlowOrigin::Source),
                Params::Route(_) => ("route parameter", Kind::Value, FlowOrigin::Source),
                Params::Rpc => ("RPC argument", Kind::Keys, FlowOrigin::Rpc(names.len())),
            };
            names.push(name.to_owned());
            let steps = vec![Spanned {
                value: format!("{label} '{name}'"),
                pos: self.loc.at(param),
            }];
            let safe = Safe::default();
            self.state.vars.insert(
                name.to_owned(),
                Taint {
                    kind,
                    origin,
                    steps,
                    safe,
                    named: false,
                    own: true,
                    upload: None,
                    origins: vec![],
                },
            );
        }
        names
    }

    fn body(&mut self, body: &[Stmt]) {
        for stmt in body {
            self.stmt(stmt);
        }
    }

    /// Runs `f` on a copy of the state and merges what it leaves into the state before it.
    fn branch(&mut self, f: impl FnOnce(&mut Self)) {
        let before = self.state.clone();
        f(self);
        let after = std::mem::replace(&mut self.state, before);
        self.state.merge(after);
    }

    /// A loop body runs twice, so that a value tainted late in it reaches the code above. `enter`
    /// runs before each pass, as a new iteration starts. The loop may stop after either pass, and
    /// `enter` may drop what the first left on the target, so both reach the code after it.
    fn repeat(&mut self, body: &[Stmt], mut enter: impl FnMut(&mut Self)) {
        self.branch(|f| {
            enter(f);
            f.body(body);
            f.branch(|g| {
                enter(g);
                g.body(body);
            });
        });
    }

    fn stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Assign(a) => {
                let t = self.assigned(&a.targets, &a.value);
                for target in &a.targets {
                    self.bind(target, t.clone(), Some(&a.value));
                }
            }
            Stmt::AnnAssign(ast::StmtAnnAssign {
                target,
                value: Some(value),
                ..
            }) => {
                let t = self.assigned(std::slice::from_ref(target), value);
                self.bind(target, t, Some(value));
            }
            Stmt::AugAssign(a) => {
                let merged = self.merged_vals(a);
                let t = join(self.expr(&a.target), self.expr(&a.value));
                self.bind(&a.target, values(t), None);
                if let Some((name, vals)) = merged {
                    self.state.vals.insert(name, vals);
                }
            }
            Stmt::Expr(e) => {
                self.expr(&e.value);
            }
            Stmt::Return(ast::StmtReturn { value: Some(v), .. }) => {
                if let Some(t) = self.expr(v) {
                    self.edges(t, &FlowTarget::Return);
                }
                self.exposed(v);
            }
            Stmt::If(i) => self.if_stmt(i),
            _ => self.compound(stmt),
        }
    }

    fn compound(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::For(f) => self.for_stmt(f),
            Stmt::While(w) => {
                self.expr(&w.test);
                self.repeat(&w.body, |_| {});
                self.body(&w.orelse);
            }
            Stmt::With(w) => self.with_stmt(w),
            Stmt::Try(t) => self.try_stmt(t),
            Stmt::Assert(a) => {
                self.test(&a.test);
                self.guard(&a.test, CmpOp::In);
            }
            _ => {}
        }
    }

    fn if_stmt(&mut self, i: &ast::StmtIf) {
        let before = self.state.clone();
        let mut joined = None;
        self.arm(Some(&i.test), &[], &i.body, &before, &mut joined);
        let mut earlier = vec![&*i.test];
        for clause in &i.elif_else_clauses {
            self.arm(clause.test.as_ref(), &earlier, &clause.body, &before, &mut joined);
            earlier.extend(&clause.test);
        }
        if i.elif_else_clauses.last().is_none_or(|c| c.test.is_some()) {
            self.state = before.clone();
            for test in earlier {
                self.holds(test, CmpOp::NotIn);
            }
            if i.elif_else_clauses.is_empty() && exits(&i.body) {
                self.guard(&i.test, CmpOp::NotIn);
            }
            let rest = std::mem::take(&mut self.state);
            merge_into(&mut joined, rest);
        }
        self.state = joined.unwrap_or(before);
    }

    /// Runs one branch of an `if` from the state before it, where each `earlier` test failed; a
    /// branch that leaves the function or the loop adds nothing to the state after it. Only its
    /// own test vouches, as a check on one path vouches on the merged one.
    fn arm(
        &mut self,
        test: Option<&Expr>,
        earlier: &[&Expr],
        body: &[Stmt],
        before: &State,
        joined: &mut Option<State>,
    ) {
        self.state = before.clone();
        for failed in earlier {
            self.holds(failed, CmpOp::NotIn);
        }
        if let Some(test) = test {
            self.test(test);
            self.vouch(self.tested(test, CmpOp::In));
            self.confine(test, CmpOp::In);
            self.holds(test, CmpOp::In);
        }
        self.body(body);
        if !exits(body) {
            let after = std::mem::take(&mut self.state);
            merge_into(joined, after);
        }
    }

    /// Reads the test of an `if` or an `assert`.
    fn test(&mut self, test: &Expr) {
        let testing = std::mem::replace(&mut self.testing, true);
        self.expr(test);
        self.testing = testing;
    }

    /// Applies a check that holds from here on: `x in allowed` leaves `x` harmless, and a test
    /// that every key of a mapping is in `allowed` takes the choice of fields away from the user,
    /// whatever `allowed` holds. `op` is the comparison the check holds for: `in` after an
    /// `assert` or in a comprehension, `not in` after an `if` that raises.
    fn guard(&mut self, test: &Expr, op: CmpOp) {
        self.vouch(self.tested(test, op));
        self.confine(test, op);
        self.allow(test, op, false);
    }

    /// Applies each membership test that holds wherever `test` holds for `op`, as `holding` reads
    /// them: a test under an `or` that is true, or an `and` that is false, may itself fail.
    /// Only a membership test counts, bare or under `not`: one compared again, as
    /// `(x in A) is False`, may hold where `x` is not in `A`.
    fn holds(&mut self, test: &Expr, op: CmpOp) {
        for t in holding(test, op) {
            if matches!(comparison(t), Some((_, CmpOp::In | CmpOp::NotIn))) {
                self.allow(t, op, true);
            }
        }
    }

    /// Confines the values and keys whose membership `test` checks for `op`, marks those it
    /// checks the start of, and those found in `EXISTING`. In an `arm`, a value or a key is
    /// confined only to a container `Flows::chosen` reads as the code's. A key found in `_fields`
    /// ends OD704 whatever records hold it.
    fn allow(&mut self, test: &Expr, op: CmpOp, arm: bool) {
        let mut checks = Checks::new(op);
        checks.visit_expr(test);
        if arm {
            checks.values.retain(|(_, container)| self.chosen(container));
            checks
                .keys
                .retain(|(_, container)| container.is_none_or(|c| self.chosen(c)));
        }
        for (name, _) in checks.values {
            self.state.vars.remove(&name);
        }
        for (name, _) in checks.keys {
            if let Some(t) = self.state.vars.get_mut(&name) {
                t.each_mut(|t| t.kind = Kind::Value);
            }
        }
        for name in checks.starts {
            if let Some(t) = self.state.vars.get_mut(&name) {
                t.each_mut(|t| t.safe = t.safe.union(Safe::of(FIXED)));
            }
        }
        for name in checks.exists {
            if let Some(t) = self.state.vars.get_mut(&name) {
                t.each_mut(|t| t.safe = t.safe.union(existing()));
            }
        }
    }

    /// Binds the target again before each pass: the second pass reads a new element, without what
    /// the first pass set on the one before it.
    fn for_stmt(&mut self, f: &ast::StmtFor) {
        let t = values(self.expr(&f.iter));
        self.repeat(&f.body, |s| s.bind(&f.target, t.clone(), None));
        self.body(&f.orelse);
    }

    fn with_stmt(&mut self, w: &ast::StmtWith) {
        for item in &w.items {
            let t = self.expr(&item.context_expr);
            if let Some(target) = &item.optional_vars {
                self.bind(target, t, None);
                // A client's `__enter__` returns the client itself.
                if let Some(key) = var_key(target) {
                    self.bind_client(&key, Some(&item.context_expr));
                }
            }
        }
        self.body(&w.body);
    }

    fn try_stmt(&mut self, t: &ast::StmtTry) {
        self.body(&t.body);
        for handler in &t.handlers {
            let ast::ExceptHandler::ExceptHandler(h) = handler;
            self.branch(|s| s.body(&h.body));
        }
        self.body(&t.orelse);
        self.body(&t.finalbody);
    }

    /// Assigns `t` to a target. A plain name or an attribute of `self` loses what it held; an item
    /// or attribute set on either adds the value to it.
    fn bind(&mut self, target: &Expr, t: Option<Taint>, value: Option<&Expr>) {
        if let Some(key) = self_attr(target) {
            self.bind_client(&key, value);
            return self.set(key, target, t);
        }
        match target {
            Expr::Name(n) => self.bind_name(n.id.as_str(), target, t, value),
            Expr::Tuple(ast::ExprTuple { elts, .. }) | Expr::List(ast::ExprList { elts, .. }) => {
                for elt in elts {
                    self.bind(elt, values(t.clone()), None);
                }
            }
            Expr::Starred(s) => self.bind(&s.value, t, None),
            // `response.headers[name] = t` sets a header, as `headers.set(name, t)` does.
            Expr::Subscript(ast::ExprSubscript { value: base, slice, .. })
                if headers_of(base).is_some_and(|owner| !self.is_client(owner)) =>
            {
                let label = header_label(self.header_name(slice), &self.path(base).join("."));
                self.sink(Sink::Header, label, t, target);
            }
            Expr::Subscript(ast::ExprSubscript { value: base, slice, .. }) => {
                self.changed(base);
                self.set_item(base, str_lit(slice), t.clone(), value);
                self.hold(base, value);
                self.add_to(base, values(t));
            }
            Expr::Attribute(ast::ExprAttribute { value: base, .. }) => {
                self.hold(base, value);
                let t = self.set_on(base, values(t));
                self.add_to(base, t);
            }
            _ => {}
        }
    }

    /// What setting an attribute on `base` adds to it. On a local a call returned, only a value the
    /// user sets on entry, without the origins joined into it: the attribute is one part of what
    /// the callee built, and an edge from a parameter or call set on it would reach every other
    /// part a sink reads, as `stream.etag` would reach `open(stream.path)`.
    fn set_on(&self, base: &Expr, t: Option<Taint>) -> Option<Taint> {
        let Expr::Name(n) = base else { return t };
        let returned = self
            .state
            .vars
            .get(n.id.as_str())
            .is_some_and(|mine| matches!(mine.origin, FlowOrigin::Return(_)));
        if !returned {
            return t;
        }
        t.filter(|t| t.origin.is_user()).map(|mut t| {
            t.origins.clear();
            t
        })
    }

    /// Binds the attachment values a local's value holds, the flow of their content going through
    /// the local.
    fn bind_vals(&mut self, name: &str, target: &Expr, value: Option<&Expr>) {
        let Some(mut vals) = value.and_then(|v| self.vals_of(v)) else {
            self.state.vals.remove(name);
            return;
        };
        vals.step(&Spanned {
            value: name.to_owned(),
            pos: self.loc.at(target),
        });
        self.state.vals.insert(name.to_owned(), vals);
    }

    /// The attachment values a local holds after `name |= right`, a mapping on the right: those it
    /// held, or else those `seed` gives it, with what `right` sets over them. `None` for any other
    /// augmented assignment, which drops them.
    fn merged_vals(&mut self, a: &ast::StmtAugAssign) -> Option<(String, Vals)> {
        let Expr::Name(n) = &*a.target else { return None };
        if a.op != Operator::BitOr || matches!(&*a.value, Expr::List(_) | Expr::Tuple(_)) {
            return None;
        }
        let name = n.id.as_str();
        let held = self.state.vals.get(name).cloned();
        let mut right = match held {
            Some(_) => self.spread(&a.value),
            None => self.vals_of(&a.value)?,
        };
        right.step(&Spanned {
            value: name.to_owned(),
            pos: self.loc.at(&*a.target),
        });
        let left = held.unwrap_or_else(|| self.seed(name));
        Some((name.to_owned(), left.over(right)))
    }

    /// The values of a local without them that an item set or `|=` adds to: any key of the content
    /// when it holds a mapping the user sends, as `**post` or `request.params` and their copies.
    /// A helper's parameter is left out, as its caller picks the keys, and so is a value derived
    /// from a mapping, which holds the code's keys.
    fn seed(&self, name: &str) -> Vals {
        let any = self
            .state
            .vars
            .get(name)
            .filter(|t| t.kind == Kind::Keys && t.origin.is_user())
            .map(|t| {
                let mut t = t.clone();
                t.origins.retain(|o| o.origin.is_user());
                t
            });
        Vals { any, ..Vals::default() }
    }

    /// Records `vals[key] = value`, of taint `t`, on the attachment values the local `base` holds.
    fn set_item(&mut self, base: &Expr, key: Option<&str>, t: Option<Taint>, value: Option<&Expr>) {
        let (Expr::Name(n), Some(key)) = (base, key) else {
            return;
        };
        let content = self.is_content(key);
        if key != "public" && !content {
            return;
        }
        let pos = self.loc.at(base);
        let name = n.id.as_str();
        // `bind` calls this before `add_to`, so the seed reads the local's own taint.
        if !self.state.vals.contains_key(name) {
            let seed = self.seed(name);
            self.state.vals.insert(name.to_owned(), seed);
        }
        let Some(vals) = self.state.vals.get_mut(name) else {
            return;
        };
        if !content {
            vals.set_public(value.is_some_and(is_true));
            return;
        }
        let t = t.map(|mut t| {
            t.step(&Spanned {
                value: n.id.to_string(),
                pos,
            });
            t
        });
        vals.set(key.to_owned(), t, true);
    }

    /// Whether the key of `ir.attachment` values holds the content an attachment stores. `datas`
    /// counts when the series is unknown, as most modules without one predate saas~19.3.
    fn is_content(&self, key: &str) -> bool {
        match key {
            "raw" | "db_datas" => true,
            "datas" => self.series.is_none_or(|s| s < NO_DATAS),
            _ => false,
        }
    }

    /// The attachment values `expr` holds: a dict literal, `dict(...)`, a list or a tuple of them,
    /// or a local bound to them or its `copy()`. Each element of a list is its own attachment, so
    /// only the public ones count when one is.
    fn vals_of(&mut self, expr: &Expr) -> Option<Vals> {
        match expr {
            Expr::Name(n) => self.state.vals.get(n.id.as_str()).cloned(),
            Expr::Dict(d) => Some(self.dict_vals(d)),
            Expr::Call(c) => match &*c.func {
                Expr::Name(n) if n.id.as_str() == "dict" => Some(self.call_vals(c)),
                Expr::Attribute(a) if a.attr.as_str() == "copy" && c.arguments.is_empty() => self.vals_of(&a.value),
                _ => None,
            },
            Expr::List(ast::ExprList { elts, .. }) | Expr::Tuple(ast::ExprTuple { elts, .. }) => self.list_vals(elts),
            _ => None,
        }
    }

    fn list_vals(&mut self, elts: &[Expr]) -> Option<Vals> {
        let all: Vec<Vals> = elts.iter().filter_map(|e| self.vals_of(e)).collect();
        let public = all.iter().any(|v| v.public);
        all.into_iter().filter(|v| v.public || !public).reduce(Vals::and)
    }

    /// The values a dict literal sets, a later key over an earlier one.
    fn dict_vals(&mut self, d: &ast::ExprDict) -> Vals {
        let mut vals = Vals::default();
        for item in &d.items {
            match &item.key {
                Some(key) => self.entry(&mut vals, str_lit(key), &item.value),
                None => vals = vals.over(self.spread(&item.value)),
            }
        }
        vals
    }

    /// The values `dict(x, key=value)` sets.
    fn call_vals(&mut self, c: &ast::ExprCall) -> Vals {
        let first = c.arguments.args.first();
        let mut vals = first.map_or_else(Vals::default, |x| self.spread(x));
        for k in &c.arguments.keywords {
            match &k.arg {
                Some(key) => self.entry(&mut vals, Some(key.as_str()), &k.value),
                None => vals = vals.over(self.spread(&k.value)),
            }
        }
        vals
    }

    /// Sets one key of attachment values: `public` to whether it is literally `True`, and a key of
    /// the content to the taint of its value.
    fn entry(&mut self, vals: &mut Vals, key: Option<&str>, value: &Expr) {
        match key {
            Some("public") => vals.set_public(is_true(value)),
            Some(key) if self.is_content(key) => {
                let t = self.expr(value);
                vals.set(key.to_owned(), t, true);
            }
            _ => {}
        }
    }

    /// What `**x` or `dict(x)` sets: the values `x` holds, or else any key of the content with the
    /// taint of `x`.
    fn spread(&mut self, x: &Expr) -> Vals {
        match self.vals_of(x) {
            Some(vals) => vals,
            None => Vals {
                any: self.expr(x),
                ..Vals::default()
            },
        }
    }

    /// Binds a local, and what the value tells of it beyond its taint.
    fn bind_name(&mut self, name: &str, target: &Expr, t: Option<Taint>, value: Option<&Expr>) {
        match value.and_then(|v| self.choice(v)) {
            Some(collection) => self.state.chosen.insert(name.to_owned(), collection),
            None => self.state.chosen.remove(name),
        };
        self.locals.insert(name.to_owned());
        // First, so that an id read from the local reads it as it was before this binding.
        self.bind_records(name, target, value);
        let sudo = value.filter(|v| self.is_sudo(v)).map(|v| {
            let mut path = self.path(v);
            path.retain(|p| p == "sudo()" || p == UID_1);
            path
        });
        match sudo {
            Some(path) => self.state.sudo.insert(name.to_owned(), path),
            None => self.state.sudo.remove(name),
        };
        let records = value.is_some_and(|v| self.records(v));
        toggle(&mut self.state.records, name, records);
        match value.and_then(|v| self.receiver(v)) {
            Some(r) => self.state.models.insert(name.to_owned(), r),
            None => self.state.models.remove(name),
        };
        match value.and_then(xml_parser) {
            Some(e) => self.state.parsers.insert(name.to_owned(), e),
            None => self.state.parsers.remove(name),
        };
        match value.and_then(|v| self.named(v)) {
            Some(named) => self.state.names.insert(name.to_owned(), named.to_owned()),
            None => self.state.names.remove(name),
        };
        match value.and_then(|v| self.picked(v)) {
            Some(model) => self.state.picked.insert(name.to_owned(), model),
            None => self.state.picked.remove(name),
        };
        match value.and_then(|v| self.header_name(v)).map(str::to_owned) {
            Some(header) => self.state.headers.insert(name.to_owned(), header),
            None => self.state.headers.remove(name),
        };
        self.bind_vals(name, target, value);
        self.bind_path(name, value);
        self.bind_client(name, value);
        self.set(name.to_owned(), target, t);
    }

    fn bind_client(&mut self, key: &str, value: Option<&Expr>) {
        toggle(&mut self.state.clients, key, value.is_some_and(client_call));
    }

    /// Whether `expr` is an outgoing client, whose headers go out with the requests it sends: a
    /// call of `CLIENTS`, a local or attribute of `self` bound to one, or a name with `session` in
    /// it, which stands for a session bound in another method, as `__init__`.
    fn is_client(&self, expr: &Expr) -> bool {
        client_call(expr) || var_key(expr).is_some_and(|k| self.state.clients.contains(&k)) || is_session(expr)
    }

    /// The last name of what `expr` refers to, through a local bound to it and `getattr()` with a
    /// literal name, so that `Loader = getattr(yaml, "CSafeLoader", yaml.SafeLoader)` is a safe
    /// loader.
    fn named<'e>(&'e self, expr: &'e Expr) -> Option<&'e str> {
        match expr {
            Expr::Name(n) => Some(
                self.state
                    .names
                    .get(n.id.as_str())
                    .map_or(n.id.as_str(), String::as_str),
            ),
            Expr::Call(c) if tail_name(&c.func) == Some("getattr") => c.arguments.args.get(1).and_then(str_lit),
            e => tail_name(e),
        }
    }

    fn set(&mut self, key: String, target: &Expr, t: Option<Taint>) {
        match t {
            Some(mut t) => {
                t.step(&Spanned {
                    value: key.clone(),
                    pos: self.loc.at(target),
                });
                self.state.vars.insert(key, t);
            }
            None => {
                self.state.vars.remove(&key);
            }
        }
    }

    /// Adds a tainted value to the local or attribute of `self` that `base` names, as
    /// `vals["x"] = t` or `vals.update(t)` do. `self` itself stays clean, so that the methods
    /// called on it are still followed.
    fn add_to(&mut self, base: &Expr, t: Option<Taint>) {
        let name = match base {
            Expr::Name(n) if !matches!(n.id.as_str(), "self" | "cls") => n.id.to_string(),
            _ => match self_attr(base) {
                Some(key) => key,
                None => return,
            },
        };
        let Some(t) = t else { return };
        // The header the user set may no longer be the only one.
        self.state.headers.remove(&name);
        let mine = self.state.vars.remove(&name);
        if let Some(t) = join(mine, Some(t)) {
            self.state.vars.insert(name, t);
        }
    }

    /// The dotted path of a callee or attribute, a call ending a name with `()` and an item
    /// access adding `[]`. A local bound under `sudo()` reads as the segments that make it so, so
    /// that a `sudo()` segment on a path, as `runs_as_superuser` reads it, means the superuser.
    fn path(&self, expr: &Expr) -> Vec<String> {
        match expr {
            Expr::Name(n) => match self.state.sudo.get(n.id.as_str()) {
                Some(path) => path.clone(),
                None => vec![n.id.to_string()],
            },
            Expr::Attribute(a) => {
                let mut path = self.path(&a.value);
                path.push(a.attr.to_string());
                path
            }
            Expr::Call(c) => self.call_path(c),
            Expr::Subscript(s) => {
                let mut path = self.path(&s.value);
                path.push("[]".to_owned());
                path
            }
            _ => vec![],
        }
    }

    /// Whether `sudo()` takes a flag in the module's series, an unknown one read as the newest.
    fn flag_sudo(&self) -> bool {
        self.series.is_none_or(|s| s >= SUDO_FLAG)
    }

    /// The path of a call. `sudo(flag)` sets `su` to its flag and `with_user()` clears it (18.0
    /// `odoo/models.py:6331`, `:6343`; 20.0 `odoo/orm/models.py:5464`, `:5474`), so a call leaving
    /// superuser mode drops the `sudo()` segments before it, and one `superuser::switch` labels,
    /// which builds a superuser environment, is followed by one, behind `UID_1` for uid 1.
    fn call_path(&self, c: &ast::ExprCall) -> Vec<String> {
        let mut path = self.path(&c.func);
        let suffix = call_suffix(c);
        if let Some(last) = path.last_mut() {
            last.push_str(suffix);
        }
        let root = is_root(c);
        let user = !root && tail_name(&c.func) == Some("with_user");
        if suffix == "(False)" || user {
            let last = path.pop();
            leave(&mut path, user || !self.flag_sudo());
            path.extend(last);
        }
        if root {
            path.push(UID_1.to_owned());
        }
        if root || superuser::switch(c).is_some() {
            path.push("sudo()".to_owned());
        }
        path
    }

    /// Whether `expr` holds records or an environment as superuser: a call or a local, or an item
    /// of an environment or of a superuser local, as `request.env(su=True)[t]` is.
    fn is_sudo(&self, expr: &Expr) -> bool {
        let item = |s: &ast::ExprSubscript| {
            is_env(&s.value) || matches!(&*s.value, Expr::Name(n) if self.state.sudo.contains_key(n.id.as_str()))
        };
        let held = match expr {
            Expr::Call(_) | Expr::Name(_) => true,
            Expr::Subscript(s) => item(s),
            _ => false,
        };
        held && runs_as_superuser(&self.path(expr))
    }

    /// Whether `expr` is records rather than what one of their methods returns: a model of an
    /// environment, a local bound to records, or a call of a method that returns records.
    fn records(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Name(n) => self.state.records.contains(n.id.as_str()),
            Expr::Subscript(s) => is_env(&s.value),
            Expr::Call(c) => tail_name(&c.func).is_some_and(|m| {
                SAME_MODEL.contains(&m)
                    || SAME_RECORDS.contains(&m)
                    || matches!(m, "search" | "search_fetch" | "sorted")
            }),
            _ => false,
        }
    }

    fn source(&self, label: String, expr: &Expr, kind: Kind) -> Option<Taint> {
        let upload = (label == FILES).then_some(Upload::Mapping);
        let steps = vec![Spanned {
            value: label,
            pos: self.loc.at(expr),
        }];
        Some(Taint {
            kind,
            origin: FlowOrigin::Source,
            steps,
            safe: Safe::default(),
            named: false,
            own: false,
            upload,
            origins: vec![],
        })
    }

    /// The methods a call on `expr` runs, when a summary may tell what they do: `self`, a model
    /// of an environment, or a local bound to one, through methods that keep the model such as
    /// `sudo()`.
    fn receiver(&self, expr: &Expr) -> Option<Receiver> {
        match expr {
            Expr::Name(n) if n.id.as_str() == "self" => self.owner.clone(),
            Expr::Name(n) => self.state.models.get(n.id.as_str()).cloned(),
            Expr::Subscript(sub) if is_env(&sub.value) => str_lit(&sub.slice).map(|m| Receiver::Model(m.to_owned())),
            Expr::Call(c) => match &*c.func {
                Expr::Attribute(a) if SAME_MODEL.contains(&a.attr.as_str()) => self.receiver(&a.value),
                _ => None,
            },
            _ => None,
        }
    }

    /// Keeps an edge to `target` for each value `Taint::split` gives.
    fn edges(&mut self, t: Taint, target: &FlowTarget) {
        for t in t.split() {
            self.edge(t, target.clone());
        }
    }

    fn edge(&mut self, t: Taint, target: FlowTarget) {
        if let Some(e) = self
            .edges
            .iter_mut()
            .find(|e| e.origin == t.origin && e.target == target)
        {
            // Harmless at the target only if it is so on every path that gets there.
            e.safe = e.safe.intersect(t.safe);
            e.upload &= t.upload == Some(Upload::Picked);
            e.own &= t.own;
            // A held join reaches its sink after the sinks below it, though it runs first.
            let sink_at = |steps: &[Spanned<String>]| steps.last().map(|s| s.pos);
            let earlier = matches!(target, FlowTarget::Sink(_)) && sink_at(&t.steps) < sink_at(&e.steps);
            if (e.named && !t.named) || (e.named == t.named && earlier) {
                e.named = t.named;
                e.steps = t.steps;
            }
            return;
        }
        self.edges.push(FlowEdge {
            origin: t.origin,
            target,
            keys: t.kind == Kind::Keys,
            safe: t.safe,
            upload: t.upload == Some(Upload::Picked),
            named: t.named,
            own: t.own,
            steps: t.steps,
        });
    }

    /// Records a call to a method the index may resolve, with the arguments that carry taint,
    /// and returns what it may return.
    fn method_call(
        &mut self,
        call: &ast::ExprCall,
        expr: &Expr,
        args: Vec<Option<Taint>>,
        keywords: Vec<Option<Taint>>,
    ) -> Option<Taint> {
        let Expr::Attribute(a) = &*call.func else { return None };
        let receiver = self.receiver(&a.value)?;
        let method = a.attr.to_string();
        let pos = self.loc.at(expr);
        let runs_as = runs_as(&self.path(&a.value));
        let site = CallSite {
            receiver,
            method,
            pos,
            runs_as,
        };
        let c = match self.calls.iter().position(|s| *s == site) {
            Some(c) => c,
            None => {
                self.calls.push(site);
                self.calls.len() - 1
            }
        };
        let slots = (0..args.len()).map(ArgSlot::Pos).chain(
            call.arguments
                .keywords
                .iter()
                .map(|k| ArgSlot::Name(k.arg.as_ref().map(|a| a.to_string()).unwrap_or_default())),
        );
        for (slot, t) in slots.zip(args.into_iter().chain(keywords)) {
            if let Some(t) = t {
                self.edges(t, &FlowTarget::Arg(c, slot));
            }
        }
        let steps = vec![Spanned {
            value: format!("{}()", a.attr),
            pos,
        }];
        Some(Taint {
            kind: Kind::Keys,
            origin: FlowOrigin::Return(c),
            steps,
            safe: Safe::default(),
            named: false,
            own: false,
            upload: None,
            origins: vec![],
        })
    }

    /// The taint of a value assigned to `targets`, holding the `os.path.join()` that builds it when
    /// it is bound to one local, bare or in normalizers, until a check of its start may confine it.
    fn assigned(&mut self, targets: &[Expr], value: &Expr) -> Option<Taint> {
        let local = matches!(targets, [Expr::Name(_)]);
        self.held_join = local.then(|| self.join_site(value)).flatten();
        let copy = local && self.held_join.is_none() && self.sites_of(value).is_some();
        let quiet = std::mem::replace(&mut self.quiet, copy);
        let t = self.expr(value);
        self.quiet = quiet;
        self.held_join = None;
        t
    }

    /// The position of the built-in `os.path.join()` that `value` is, or that normalizers take.
    fn join_site(&self, value: &Expr) -> Option<Pos> {
        let Expr::Call(c) = value else { return None };
        if let Some(path) = self.normalized(c) {
            return self.join_site(path);
        }
        let row = self.table.lookup(&self.path(&c.func));
        matches!(row, Some(("os.path.join", Role::Sink(r))) if r.sink == Sink::Path).then(|| self.loc.at(value))
    }

    /// The path a call of a normalizer takes.
    fn normalized<'e>(&self, c: &'e ast::ExprCall) -> Option<&'e Expr> {
        let path = self.path(&c.func);
        let normalizer = NORMALIZERS.iter().any(|n| matches_path(n, &path));
        c.arguments.args.first().filter(|_| normalizer)
    }

    /// The joins of `Flows::held` that `value` holds: a held join itself, a normalizer of one, or a
    /// local bound to one.
    fn sites_of(&self, value: &Expr) -> Option<Sites> {
        match value {
            Expr::Name(n) => self.state.paths.get(n.id.as_str()).cloned(),
            Expr::Call(c) => match self.normalized(c) {
                Some(path) => self.sites_of(path).map(|s| Sites { normalized: true, ..s }),
                None => {
                    let pos = self.loc.at(value);
                    self.held.contains_key(&pos).then(|| Sites {
                        at: vec![pos],
                        normalized: false,
                        confined: false,
                    })
                }
            },
            _ => None,
        }
    }

    fn bind_path(&mut self, name: &str, value: Option<&Expr>) {
        match value.and_then(|v| self.sites_of(v)) {
            Some(sites) => self.state.paths.insert(name.to_owned(), sites),
            None => self.state.paths.remove(name),
        };
    }

    /// Reports a sink, but holds the join `Flows::assigned` holds.
    fn reach(&mut self, sink: Sink, callee: String, t: Option<Taint>, expr: &Expr) {
        let pos = self.loc.at(expr);
        if self.held_join != Some(pos) {
            return self.sink_pos(sink, callee, t, pos);
        }
        let Some(t) = t.filter(|t| !t.safe.has(code(sink))) else {
            return;
        };
        let (taint, checked) = match self.held.remove(&pos) {
            Some(kept) => (joined(kept.taint, t), kept.checked),
            None => (t, false),
        };
        self.held.insert(pos, Held { taint, callee, checked });
    }

    /// Reports the held joins a read of the local `name` uses where no check confines them.
    fn read(&mut self, name: &str) {
        let Some(sites) = self.state.paths.get(name).filter(|s| !s.confined && !self.quiet) else {
            return;
        };
        for pos in sites.at.clone() {
            if let Some(held) = self.held.remove(&pos) {
                self.sink_pos(Sink::Path, held.callee, Some(held.taint), pos);
            }
        }
    }

    /// Confines a normalized local whose start `test` holds to prefixes the code picks, where
    /// `test` holds for `op` as in `Flows::guard`: no `..` then leaves the prefix. Any prefix
    /// past the root counts, though `/srv/docs` admits `/srv/docs2`. In an `if` the state is the arm's, so a
    /// read after it, as after a test that raises on a prefix it denies, is still unchecked.
    fn confine(&mut self, test: &Expr, op: CmpOp) {
        for t in holding(test, op) {
            let Some((name, prefix)) = started(t, op) else {
                continue;
            };
            let Some(sites) = self.state.paths.get(name).filter(|s| s.normalized).cloned() else {
                continue;
            };
            if !fixed_prefixes(prefix, |p| self.picked_prefix(p)) {
                continue;
            }
            for pos in &sites.at {
                if let Some(held) = self.held.get_mut(pos) {
                    held.checked = true;
                }
            }
            if let Some(sites) = self.state.paths.get_mut(name) {
                sites.confined = true;
            }
        }
    }

    /// Whether the code picks a prefix that confines a path: the user sets none of it, no held
    /// join built it, and it is more than the root, which every absolute path starts with.
    fn picked_prefix(&mut self, prefix: &Expr) -> bool {
        let root =
            str_lit(prefix).is_some_and(|s| matches!(s, "" | "/" | "\\")) || matches_path("os.sep", &self.path(prefix));
        let siteless = self.names_in(prefix).iter().all(|n| !self.state.paths.contains_key(n));
        !root && siteless && self.expr(prefix).is_none()
    }

    /// Reports, at the call of each, the held joins no read reported and no check ever confined.
    fn report_held(&mut self) {
        let mut held: Vec<(Pos, Held)> = std::mem::take(&mut self.held).into_iter().collect();
        held.sort_by_key(|(pos, _)| *pos);
        for (pos, held) in held.into_iter().filter(|(_, h)| !h.checked) {
            self.sink_pos(Sink::Path, held.callee, Some(held.taint), pos);
        }
    }

    fn expr(&mut self, expr: &Expr) -> Option<Taint> {
        match expr {
            Expr::Name(n) => {
                self.read(n.id.as_str());
                self.state.vars.get(n.id.as_str()).cloned()
            }
            Expr::Attribute(a) => match self.table.lookup(&self.path(expr)) {
                Some((pattern, Role::Source(kind))) => self.source(pattern.to_owned(), expr, kind),
                _ => match self_attr(expr).and_then(|key| self.state.vars.get(&key)) {
                    Some(t) => Some(t.clone()),
                    None => attribute(self.expr(&a.value), a.attr.as_str()),
                },
            },
            Expr::Subscript(s) => self.item(s, expr),
            Expr::Call(c) => self.called(c, expr),
            Expr::BinOp(b) => self.bin_op(b),
            Expr::FString(f) => self.fstring(f),
            Expr::Named(n) => {
                let t = self.expr(&n.value);
                self.bind(&n.target, t.clone(), Some(&n.value));
                t
            }
            _ => self.collection(expr),
        }
    }

    /// What a call returns; a test of a path's start reads no file, so it uses no held join.
    fn called(&mut self, c: &ast::ExprCall, expr: &Expr) -> Option<Taint> {
        let quiet = self.quiet || started(expr, CmpOp::In).is_some();
        let quiet = std::mem::replace(&mut self.quiet, quiet);
        let t = self.call(c, expr);
        self.quiet = quiet;
        t
    }

    fn collection(&mut self, expr: &Expr) -> Option<Taint> {
        match expr {
            Expr::List(ast::ExprList { elts, .. }) | Expr::Tuple(ast::ExprTuple { elts, .. }) => {
                let mut ts: Vec<Option<Taint>> = elts.iter().map(|e| self.expr(e)).collect();
                hold_value(elts, matches!(expr, Expr::List(_)), &mut ts);
                ts.into_iter().fold(None, join)
            }
            Expr::Set(ast::ExprSet { elts, .. }) => self.all(elts.iter()),
            Expr::Dict(d) => self.dict(d),
            Expr::DictComp(d) => self.dict_comp(d),
            Expr::ListComp(ast::ExprListComp { elt, generators, .. })
            | Expr::SetComp(ast::ExprSetComp { elt, generators, .. })
            | Expr::Generator(ast::ExprGenerator { elt, generators, .. }) => {
                self.comprehension(generators, |s| values(s.expr(elt)))
            }
            _ => self.operator(expr),
        }
    }

    fn operator(&mut self, expr: &Expr) -> Option<Taint> {
        match expr {
            Expr::BoolOp(b) => self.all(b.values.iter()),
            Expr::If(i) => {
                self.expr(&i.test);
                let body = self.expr(&i.body);
                join(body, self.expr(&i.orelse))
            }
            Expr::Starred(s) => self.expr(&s.value),
            Expr::Await(a) => self.expr(&a.value),
            Expr::Compare(c) => {
                self.all(c.operands.iter());
                None
            }
            Expr::UnaryOp(u) => {
                let t = self.expr(&u.operand);
                // `~` of a domain negates its conditions, which the user still picks.
                if u.op == ast::UnaryOp::Invert && is_domain(&u.operand) {
                    t
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    fn all<'e>(&mut self, exprs: impl Iterator<Item = &'e Expr>) -> Option<Taint> {
        exprs.fold(None, |acc, e| {
            let t = self.expr(e);
            join(acc, t)
        })
    }

    fn bin_op(&mut self, b: &ast::ExprBinOp) -> Option<Taint> {
        let left = self.expr(&b.left);
        let (right, first) = self.right_operand(b);
        // `Markup("<b>%s</b>") % value` escapes the value.
        if b.op == Operator::Mod && self.is_markup(&b.left) {
            return None;
        }
        // An intersection with values the code chose holds only those values, but `&` of two
        // domains joins their conditions as `expression.AND` does.
        let chosen = |t: &Option<Taint>| t.as_ref().is_none_or(|t| matches!(t.origin, FlowOrigin::Return(_)));
        let domains = is_domain(&b.left) || is_domain(&b.right);
        if b.op == Operator::BitAnd && !domains && (chosen(&left) || chosen(&right)) {
            return None;
        }
        let start = bin_start(b, left.as_ref(), first).union(bin_affix(b));
        let t = join(left, right);
        let t = if b.op == Operator::BitOr { t } else { values(t) };
        t.map(|t| t.safe(start))
    }

    /// The taint of the right operand, and the first value it puts in a `%` template with its own.
    fn right_operand<'e>(&mut self, b: &'e ast::ExprBinOp) -> (Option<Taint>, Option<(&'e Expr, Option<Taint>)>) {
        match &*b.right {
            Expr::Tuple(tuple) if b.op == Operator::Mod => {
                let ts: Vec<Option<Taint>> = tuple.elts.iter().map(|e| self.expr(e)).collect();
                let first = tuple.elts.first().zip(ts.first().cloned());
                (ts.into_iter().fold(None, join), first)
            }
            right => {
                let t = self.expr(right);
                (t.clone(), Some((right, t)))
            }
        }
    }

    /// Only the built-in row: a configured OD703 sink, such as one that marks a value safe, does
    /// not escape what `%` puts in it.
    fn is_markup(&self, expr: &Expr) -> bool {
        matches!(expr, Expr::Call(c) if built_in(&self.path(&c.func)).is_some_and(|(_, r)| matches!(r, Role::Sink(row) if row.sink == Sink::Markup)))
    }

    /// An f-string starts with the text before its first field or, when no text comes before the
    /// first field, with that field's value.
    fn fstring(&mut self, f: &ast::ExprFString) -> Option<Taint> {
        let mut text = String::new();
        // The length of the text before the first field, and where that field starts.
        let mut first: Option<(usize, Safe)> = None;
        let mut t = None;
        for part in f.value.iter() {
            let elements = match part {
                ast::FStringPartRef::Literal(l) => {
                    text.push_str(&l.value);
                    continue;
                }
                ast::FStringPartRef::FString(f) => &f.elements,
            };
            for e in elements {
                match e {
                    ast::InterpolatedStringElement::Literal(l) => text.push_str(&l.value),
                    ast::InterpolatedStringElement::Interpolation(i) => {
                        let field = self.expr(&i.expression);
                        let start = start_of(&i.expression, field.as_ref());
                        first.get_or_insert((text.len(), start));
                        t = join(t, field);
                    }
                }
            }
        }
        let start = match first {
            Some((0, start)) => start,
            Some((len, _)) => literal_start(&text[..len]),
            None => literal_start(&text),
        };
        values(t).map(|t| t.safe(start).safe(affixed(&text)))
    }

    /// `{**t}` keeps the keys of `t`; a literal key set holds the user to the values.
    fn dict(&mut self, d: &ast::ExprDict) -> Option<Taint> {
        d.items.iter().fold(None, |acc, item| {
            let key = item.key.as_ref().map(|k| self.expr(k));
            let value = self.expr(&item.value);
            let t = match key {
                None => value,
                Some(key) => join(key.map(|t| t.kind(Kind::Keys)), values(value)),
            };
            join(acc, t)
        })
    }

    /// A comprehension whose keys come from the user names any field, unless an `if` filters it.
    fn dict_comp(&mut self, d: &ast::ExprDictComp) -> Option<Taint> {
        let filtered = d.generators.iter().any(|g| !g.ifs.is_empty());
        self.comprehension(&d.generators, |s| {
            let key = d.key.as_ref().and_then(|k| s.expr(k));
            let value = values(s.expr(&d.value));
            let key = key.map(|t| t.kind(if filtered { Kind::Value } else { Kind::Keys }));
            join(key, value)
        })
    }

    fn comprehension(
        &mut self,
        generators: &[ast::Comprehension],
        elt: impl FnOnce(&mut Self) -> Option<Taint>,
    ) -> Option<Taint> {
        let before = self.state.clone();
        for g in generators {
            let t = values(self.expr(&g.iter));
            self.bind(&g.target, t, None);
            self.all(g.ifs.iter());
            // The element is built only where each test holds, which says nothing of a
            // comparison under `or` or `not`.
            for test in g.ifs.iter().flat_map(conjuncts) {
                if matches!(test, Expr::Compare(_)) {
                    self.guard(test, CmpOp::In);
                } else {
                    self.confine(test, CmpOp::In);
                }
            }
        }
        let t = elt(self);
        self.state = before;
        t
    }

    /// The taints of a call's positional arguments. The list `expression.AND()`, `Domain.OR()` or
    /// an `AND()` or `OR()` imported from `odoo.osv.expression` takes holds domains, and the field
    /// list of `search_read()` or `search_fetch()` holds names: neither is ever one condition.
    fn args(&mut self, call: &ast::ExprCall) -> Vec<Option<Taint>> {
        let joins = is_domain_join(&call.func, self.domain_joins, &self.locals);
        let fields = field_names(call);
        let args = call.arguments.args.iter();
        args.map(|a| match a {
            Expr::List(ast::ExprList { elts, .. }) | Expr::Tuple(ast::ExprTuple { elts, .. })
                if joins || fields.is_some_and(|f| std::ptr::eq(f, a)) =>
            {
                self.all(elts.iter())
            }
            a => self.expr(a),
        })
        .collect()
    }

    /// The taints of a call's keyword arguments, the field names of a search read item by item.
    fn keywords(&mut self, call: &ast::ExprCall) -> Vec<Option<Taint>> {
        let fields = field_names(call);
        let keywords = call.arguments.keywords.iter();
        keywords
            .map(|k| match &k.value {
                Expr::List(ast::ExprList { elts, .. }) | Expr::Tuple(ast::ExprTuple { elts, .. })
                    if fields.is_some_and(|f| std::ptr::eq(f, &k.value)) =>
                {
                    self.all(elts.iter())
                }
                v => self.expr(v),
            })
            .collect()
    }

    fn call(&mut self, call: &ast::ExprCall, expr: &Expr) -> Option<Taint> {
        let receiver = match &*call.func {
            Expr::Attribute(a) => self.expr(&a.value),
            _ => None,
        };
        let args = self.args(call);
        let keywords = self.keywords(call);
        self.mutates(call);
        let path = self.path(&call.func);
        // A call whose guard fails is no sink, and returns what any other call would, as
        // `etree.fromstring(t, HTMLParser())` returns the tree of `t`.
        let row = self.table.lookup(&path).filter(|(_, role)| match role {
            Role::Sink(row) => self.is_sink(*row, &path, call),
            _ => true,
        });
        self.picks(call, expr);
        self.publishes(call, expr);
        self.checks(call);
        match row {
            Some((pattern, Role::Source(kind))) => self.source(format!("{pattern}()"), expr, kind),
            Some((_, Role::Sanitizer)) => None,
            Some((pattern, Role::Sink(row))) => {
                let param = self.param(row);
                let arg = sink_arg(param, code(row.sink), call, &args, &keywords);
                let callee = match row.sink {
                    Sink::Header => header_label(
                        arg_expr(param, call).and_then(|e| self.header_name(e)),
                        &format!("{pattern}()"),
                    ),
                    _ => sink_callee(pattern, call),
                };
                self.reach(row.sink, callee, arg, expr);
                // A parser, a loader or a command hands back the data in another form, and a
                // search or a read stays a call of the model's method, so the flows that pass
                // through it go on as they would without the row.
                let passes = matches!(
                    row.sink,
                    Sink::Command | Sink::Deserialize | Sink::Entity | Sink::Header | Sink::Field | Sink::Domain
                );
                if !passes {
                    return None;
                }
                self.plain_call(call, expr, &path, receiver, args, keywords)
            }
            Some((_, role)) => transformed(role, call, args, keywords),
            None => self.plain_call(call, expr, &path, receiver, args, keywords),
        }
    }

    /// An item's value, reporting a key the user picks on records under `sudo()`, which reads any
    /// field as superuser.
    fn item(&mut self, s: &ast::ExprSubscript, expr: &Expr) -> Option<Taint> {
        let key = self.expr(&s.slice);
        if self.is_sudo(&s.value) && self.records(&s.value) {
            self.sink(Sink::Field, format!("{}[...]", superuser_call(&s.value)), key, expr);
        }
        item_of(self.expr(&s.value), str_lit(&s.slice))
    }

    /// Vouches for what a call checks, but for a boolean check in the test of an `if` or an
    /// `assert`, which vouches only where it passes.
    fn checks(&mut self, call: &ast::ExprCall) {
        if !(self.testing && is_boolean_check(call)) {
            self.vouch(self.checked_by(call));
        }
    }

    /// Reports a method the user names that the call runs, as `getattr(records, t)()`, and a data
    /// method on the records of a model the user picks under `sudo()`.
    fn picks(&mut self, call: &ast::ExprCall, expr: &Expr) {
        match &*call.func {
            Expr::Call(inner) if tail_name(&inner.func) == Some("getattr") => {
                let Some(name) = inner.arguments.args.get(1) else {
                    return;
                };
                let t = self.expr(name);
                self.sink(Sink::Method, "getattr()".to_owned(), t, expr);
            }
            Expr::Attribute(a) if is_data_method(a.attr.as_str()) && self.is_sudo(&a.value) => {
                let t = self.picked(&a.value);
                let callee = format!("{}.{}()", superuser_call(&a.value), a.attr);
                self.sink(Sink::Model, callee, t, expr);
            }
            _ => {}
        }
    }

    /// Reports content the user sets that a create of `ir.attachment` stores as superuser, in an
    /// attachment whose values set `public` to `True`, which anyone may then download.
    fn publishes(&mut self, call: &ast::ExprCall, expr: &Expr) {
        let Expr::Attribute(a) = &*call.func else { return };
        let Some((method, values, file)) = CREATES.iter().find(|(m, ..)| *m == a.attr.as_str()) else {
            return;
        };
        let attachment = matches!(self.receiver(&a.value), Some(Receiver::Model(m)) if m == "ir.attachment");
        if !attachment || runs_as(&self.path(&a.value)) != RunsAs::Superuser {
            return;
        }
        let vals = arg_expr(*values, call).and_then(|v| self.vals_of(v));
        let Some(vals) = vals.filter(|v| v.public) else { return };
        let file = file.and_then(|f| arg_expr(f, call)).and_then(|f| self.expr(f));
        self.sink(
            Sink::Attachment,
            format!("{}.{method}()", superuser_call(&a.value)),
            join(vals.content(), file),
            expr,
        );
    }

    /// The taint of the model's name when `expr` is the records of a model the user picks:
    /// `env[t]`, `env.get(t)`, a local bound to them, and the same records through `sudo()` and
    /// the like, `browse()`, `exists()` or `filtered()`.
    fn picked(&mut self, expr: &Expr) -> Option<Taint> {
        match expr {
            Expr::Name(n) => self.state.picked.get(n.id.as_str()).cloned(),
            Expr::Subscript(s) if is_env(&s.value) => self.expr(&s.slice),
            Expr::Call(c) => self.picked_call(c),
            _ => None,
        }
    }

    fn picked_call(&mut self, c: &ast::ExprCall) -> Option<Taint> {
        let Expr::Attribute(a) = &*c.func else { return None };
        match a.attr.as_str() {
            "get" if is_env(&a.value) => self.expr(c.arguments.args.first()?),
            m if SAME_MODEL.contains(&m) || SAME_RECORDS.contains(&m) => self.picked(&a.value),
            _ => None,
        }
    }

    /// Reports records browsed by an id the user picks and read as superuser that a route returns,
    /// rendered or not.
    fn exposed(&mut self, value: &Expr) {
        if !self.route {
            return;
        }
        if let Some(b) = self.browsed(value).filter(|b| b.sudo) {
            self.sink(Sink::Record, "browse()".to_owned(), Some(b.id), value);
        }
    }

    /// The records browsed by an id the user picks that `expr` holds: `browse(id)` and a local
    /// bound to them, kept by the methods that change the environment, `exists()`, `filtered()`,
    /// `read()` and `mapped()`, ended by any other of their methods, and kept by an attribute or an
    /// item when read as superuser. Any other call, a container or an expression holds those of its
    /// parts, records read as superuser first.
    fn browsed(&mut self, expr: &Expr) -> Option<Browsed> {
        match expr {
            Expr::Name(n) => self.state.browsed.get(n.id.as_str()).cloned(),
            Expr::Call(c) => self.browsed_call(c),
            Expr::Attribute(ast::ExprAttribute { value, .. }) | Expr::Subscript(ast::ExprSubscript { value, .. }) => {
                self.browsed(value).filter(|b| b.sudo)
            }
            e => self.first_browsed(parts(e)),
        }
    }

    fn browsed_call(&mut self, c: &ast::ExprCall) -> Option<Browsed> {
        let Expr::Attribute(a) = &*c.func else {
            let counted = matches!(tail_name(&c.func), Some("len" | "bool"));
            return if counted {
                None
            } else {
                self.first_browsed(arguments(c))
            };
        };
        if a.attr.as_str() == "browse" {
            return self.browse(c, &a.value);
        }
        match self.browsed(&a.value) {
            Some(b) => kept(b, c, a.attr.as_str(), self.flag_sudo()),
            None => self.first_browsed(arguments(c)),
        }
    }

    /// The records `browse()` returns on `records` when the user picks the id, unless a check
    /// vouched for each local the id names that holds a value.
    fn browse(&mut self, c: &ast::ExprCall, records: &Expr) -> Option<Browsed> {
        let keyword = || c.arguments.find_keyword("ids").map(|k| &k.value);
        let arg = c.arguments.args.first().or_else(keyword)?;
        let names = self.names_in(arg);
        if self.vouched(&names) {
            return None;
        }
        Some(Browsed {
            id: self.id_of(arg)?,
            sudo: self.is_sudo(records),
            root: self.path(records).iter().any(|p| p == UID_1),
            names,
        })
    }

    /// The taint of an id, through `int()`, `float()`, `map()`, a list, a comprehension and an `if`
    /// expression, which end other flows but still pick the records.
    fn id_of(&mut self, e: &Expr) -> Option<Taint> {
        match e {
            Expr::Name(n) => {
                let vars = self.state.vars.get(n.id.as_str());
                vars.or_else(|| self.state.ids.get(n.id.as_str())).cloned()
            }
            Expr::Call(c) if is_cast(e) => self.id_of(c.arguments.args.first()?),
            Expr::Call(c) if tail_name(&c.func) == Some("map") => self.id_of(c.arguments.args.get(1)?),
            _ => match id_parts(e) {
                Some(parts) => parts.into_iter().fold(None, |acc, p| {
                    let t = self.id_of(p);
                    join(acc, t)
                }),
                None => self.expr(e),
            },
        }
    }

    /// The records the first of `exprs` holding records read as superuser holds, or else the first
    /// holding any.
    fn first_browsed<'e>(&mut self, exprs: impl IntoIterator<Item = &'e Expr>) -> Option<Browsed> {
        let mut first = None;
        for e in exprs {
            match self.browsed(e) {
                Some(b) if b.sudo => return Some(b),
                b => first = first.or(b),
            }
        }
        first
    }

    /// Binds what a local holds of records browsed by an id the user picks: the records, the id
    /// cast by `int()` or `float()`, and the check of a local it is bound from. A local bound from
    /// held records joins their names, so that a check of it vouches for them.
    fn bind_records(&mut self, name: &str, target: &Expr, value: Option<&Expr>) {
        let named = value.map(|v| self.names_in(v)).unwrap_or_default();
        for (key, b) in &mut self.state.browsed {
            if named.contains(key) && !b.names.iter().any(|n| n == name) {
                b.names.push(name.to_owned());
            }
        }
        let checked = self.vouched(&named);
        toggle(&mut self.state.checked, name, checked);
        let step = Spanned {
            value: name.to_owned(),
            pos: self.loc.at(target),
        };
        match value.and_then(|v| self.browsed(v)) {
            Some(b) => self.state.browsed.insert(name.to_owned(), b.bound(step.clone())),
            None => self.state.browsed.remove(name),
        };
        match value.filter(|v| is_cast(v)).and_then(|v| self.id_of(v)) {
            Some(mut t) => {
                t.step(&step);
                self.state.ids.insert(name.to_owned(), t)
            }
            None => self.state.ids.remove(name),
        };
    }

    /// Adds the records `values` hold, as `Flows::first_browsed` picks them, to the local `base`
    /// names, as `vals["x"] = r` or `vals.update(x=r)` do.
    fn hold<'e>(&mut self, base: &Expr, values: impl IntoIterator<Item = &'e Expr>) {
        let Expr::Name(n) = base else { return };
        let Some(b) = self.first_browsed(values) else { return };
        let step = Spanned {
            value: n.id.to_string(),
            pos: self.loc.at(base),
        };
        let mine = self.state.browsed.remove(n.id.as_str());
        if let Some(b) = join_browsed(mine, Some(b.bound(step))) {
            self.state.browsed.insert(n.id.to_string(), b);
        }
    }

    /// The header a literal names, or that a list of pairs or a mapping, inline or bound to a
    /// local, sets in its one non-literal entry, or else in its one tainted entry, as
    /// `Content-Disposition` beside `Content-Length` at 18.0 `addons/mail/controllers/attachment.py:40-46`.
    fn header_name<'e>(&'e self, expr: &'e Expr) -> Option<&'e str> {
        if let Expr::Name(n) = expr {
            return self.state.headers.get(n.id.as_str()).map(String::as_str);
        }
        let Some(entries) = header_entries(expr) else {
            return str_lit(expr);
        };
        let mut set: Vec<(&Expr, &Expr)> = entries.into_iter().filter(|(_, v)| constant(v).is_none()).collect();
        if set.len() > 1 {
            set.retain(|(_, v)| self.reads_user(v));
        }
        match set[..] {
            [(key, _)] => str_lit(key),
            _ => None,
        }
    }

    /// Whether `expr` reads a value that would reach a header, judged without evaluating it
    /// again, since that would report its sinks twice.
    fn reads_user(&self, expr: &Expr) -> bool {
        let mut reads = Reads {
            flows: self,
            found: false,
        };
        reads.visit_expr(expr);
        reads.found
    }

    /// Whether the code chose the values of `container`, so that a membership test in it confines
    /// the value tested: a list, tuple, set or dict literal of `fixed` items, a list or set
    /// comprehension over any of these, a module constant bound to such a literal, `self._x` or `cls._x`
    /// but `MODEL_ATTRS`, what a method of `self` returns for `fixed` arguments but one that reads
    /// records or lists fields, or a local of `State::chosen` bound to any of these. Nothing else
    /// is: a string holds every substring, and a parameter, a record's values or a model's fields
    /// may be the user's.
    fn chosen(&self, container: &Expr) -> bool {
        match container {
            Expr::Name(n) if self.locals.contains(n.id.as_str()) => self.state.chosen.get(n.id.as_str()) == Some(&true),
            Expr::Name(n) => self.collections.contains(&n.id.as_str()),
            Expr::Attribute(a) => {
                of_self(&a.value) && a.attr.starts_with('_') && !MODEL_ATTRS.contains(&a.attr.as_str())
            }
            Expr::Call(c) => self.chosen_return(c),
            Expr::ListComp(ast::ExprListComp { elt, generators, .. })
            | Expr::SetComp(ast::ExprSetComp { elt, generators, .. }) => self.chosen_over(elt, generators),
            e => {
                module_state::items(e).is_some_and(|items| items.into_iter().all(|i| i.is_some_and(|i| self.fixed(i))))
            }
        }
    }

    /// Whether `call` is of a method of `self` that neither reads records nor lists fields, with
    /// `fixed` arguments.
    fn chosen_return(&self, call: &ast::ExprCall) -> bool {
        let Expr::Attribute(m) = &*call.func else { return false };
        let method = m.attr.as_str();
        let keywords = call.arguments.keywords.iter().map(|k| &k.value);
        of_self(&m.value)
            && !is_data_method(method)
            && method != "fields_get"
            && call.arguments.args.iter().chain(keywords).all(|a| self.fixed(a))
    }

    /// Whether a comprehension iterates only `chosen` collections and builds each item of a
    /// target it binds or of a `fixed` value.
    fn chosen_over(&self, elt: &Expr, generators: &[ast::Comprehension]) -> bool {
        let bound = |n: &ast::ExprName| {
            generators
                .iter()
                .any(|g| g.target.as_name_expr().is_some_and(|t| t.id == n.id))
        };
        generators.iter().all(|g| self.chosen(&g.iter)) && (elt.as_name_expr().is_some_and(bound) || self.fixed(elt))
    }

    /// Whether the code chose `item`: a literal but an f-string, a local bound to one, or a
    /// `chosen` collection.
    fn fixed(&self, item: &Expr) -> bool {
        let local = item
            .as_name_expr()
            .is_some_and(|n| self.state.chosen.contains_key(n.id.as_str()));
        module_state::scalar(item) || local || self.chosen(item)
    }

    /// What a local bound to `value` holds that the code chose: `Some(true)` for a `chosen`
    /// collection, `Some(false)` for another `fixed` value.
    fn choice(&self, value: &Expr) -> Option<bool> {
        if self.chosen(value) {
            Some(true)
        } else {
            self.fixed(value).then_some(false)
        }
    }

    /// Drops from `State::chosen` the local a call of `MUTATORS` changes.
    fn mutates(&mut self, call: &ast::ExprCall) {
        if let Expr::Attribute(a) = &*call.func
            && MUTATORS.contains(&a.attr.as_str())
        {
            self.changed(&a.value);
        }
    }

    /// Drops the local `base` names from `State::chosen`, as it took a value the code may not
    /// have chosen.
    fn changed(&mut self, base: &Expr) {
        if let Expr::Name(n) = base {
            self.state.chosen.remove(n.id.as_str());
        }
    }

    /// The locals `expr` names, but `self`.
    fn names_in(&self, expr: &Expr) -> Vec<String> {
        let mut names = Names::default();
        names.visit_expr(expr);
        names
            .0
            .retain(|n| self.locals.contains(n) && !matches!(n.as_str(), "self" | "cls"));
        names.0
    }

    /// Vouches for the records that the locals `names` hold or pick. Drops the held records that
    /// name one of them, or one of the names of the records they hold, and marks all those names
    /// checked, so that a later id naming only checked locals picks nothing for the user. A local
    /// of records that also holds records the check does not reach, as a mapping of two records
    /// does, stays out of those names: the check vouches for one of its members only.
    fn vouch(&mut self, mut names: Vec<String>) {
        if names.is_empty() {
            return;
        }
        let browsed = &self.state.browsed;
        let reached = |key: &String, b: &Browsed| names.contains(key) || b.names.iter().any(|n| names.contains(n));
        let unreached: Vec<&String> = browsed
            .iter()
            .filter(|(key, b)| !reached(key, b))
            .flat_map(|(_, b)| &b.names)
            .collect();
        let holder = |n: &String| browsed.contains_key(n) && unreached.contains(&n);
        let named: Vec<String> = names
            .iter()
            .filter_map(|n| browsed.get(n))
            .flat_map(|b| b.names.iter().filter(|n| !holder(n)).cloned())
            .collect();
        names.extend(named);
        let checked = |n: &String| names.contains(n);
        self.state
            .browsed
            .retain(|key, b| !checked(key) && !b.names.iter().any(checked));
        self.state.checked.extend(names);
    }

    /// Whether a check vouched for an id naming `names`: for one of them, and for each that holds a
    /// value, so that an unchecked part still picks the records.
    fn vouched(&self, names: &[String]) -> bool {
        let checked = |n: &String| self.state.checked.contains(n);
        let holds = |n: &&String| self.state.vars.contains_key(*n) || self.state.ids.contains_key(*n);
        names.iter().any(checked) && names.iter().filter(holds).all(checked)
    }

    /// The locals a call checks the access to: the id of a document or a token check, what a
    /// token is compared to, the arguments and records of a `_check` method, and the records of
    /// an access check on records that do not run as superuser, which skips them (18.0
    /// `odoo/models.py:4456`, `:4464`; 16.0 `:3524`), as `runs_as_superuser` reads their path.
    /// `check_access_rights()` checks the model only (18.0 `odoo/models.py:4516`).
    fn checked_by(&self, call: &ast::ExprCall) -> Vec<String> {
        let receiver = match &*call.func {
            Expr::Attribute(a) => Some(&*a.value),
            _ => None,
        };
        // A field or an item of sudo records keeps their environment (18.0 `odoo/fields.py:3341`),
        // so a `sudo()` anywhere on the path skips the check.
        let skipped = |r: &&Expr| runs_as_superuser(&self.path(r));
        let checked: Vec<&Expr> = match tail_name(&call.func).unwrap_or_default() {
            "check_access" | "check_access_rule" | "has_access" => {
                receiver.filter(|r| !skipped(r)).into_iter().collect()
            }
            "_document_check_access" => call
                .arguments
                .find_argument_value("document_id", 1)
                .into_iter()
                .collect(),
            // `check_access_token(token, *values)` signs the values, the ids among them.
            "check_access_token" => call.arguments.args.iter().skip(1).collect(),
            "consteq" | "compare_digest" => arguments(call).filter(|a| !self.sent(a)).collect(),
            n if n.starts_with("_check") && !n.starts_with("_checkout") => arguments(call).chain(receiver).collect(),
            _ => vec![],
        };
        checked.into_iter().flat_map(|a| self.names_in(a)).collect()
    }

    /// Whether `expr` is a token as the user sent it, such as `token` or `kw.get("token")`, which
    /// vouches for nothing it is compared to, unlike a value read from or computed for records.
    fn sent(&self, expr: &Expr) -> bool {
        let root = match expr {
            Expr::Subscript(s) => &*s.value,
            Expr::Call(c) if tail_name(&c.func) == Some("get") => match &*c.func {
                Expr::Attribute(a) => &*a.value,
                _ => return false,
            },
            e => e,
        };
        let held = |n: &str| {
            self.state
                .browsed
                .iter()
                .any(|(k, b)| k == n || b.names.iter().any(|m| m == n))
        };
        root.as_name_expr().is_some_and(|n| !held(n.id.as_str()))
    }

    /// The locals a test vouches for where it holds: those of a check it calls, of a boolean
    /// check that passes there, of records whose field it reads or that it compares, and `x` for
    /// `x <op> allowed` or `int(x) <op> allowed`.
    fn tested(&self, test: &Expr, op: CmpOp) -> Vec<String> {
        let mut tests = Tests {
            flows: self,
            op,
            names: vec![],
        };
        tests.visit_expr(test);
        let passed = passing(test, op).into_iter().flat_map(|c| self.checked_by(c));
        tests.names.extend(passed);
        tests.names
    }

    /// What a call returns when its row ends no flow: a call no row names, one whose row is no
    /// sink here (its guard fails, an `odoo` import binds its module, it sets a client's headers),
    /// and one whose sink passes its data on after reporting it. Keeps a write on `self` or a call
    /// of a model's method for the summary.
    fn plain_call(
        &mut self,
        call: &ast::ExprCall,
        expr: &Expr,
        path: &[String],
        receiver: Option<Taint>,
        args: Vec<Option<Taint>>,
        keywords: Vec<Option<Taint>>,
    ) -> Option<Taint> {
        self.self_write(call, &args, &keywords);
        if receiver.is_none() && self.method_target(call) {
            return self.method_call(call, expr, args, keywords);
        }
        let start = format_start(call, &args, &keywords).union(format_affix(call));
        let mut args = args;
        // A mapping's `get()` returns one of its values, whoever picks the key.
        if receiver.is_some() && path.last().is_some_and(|m| m == "get") {
            args.iter_mut().take(1).for_each(|key| *key = None);
        }
        let args = args.into_iter().chain(keywords).fold(None, join);
        let t = self.result(call, path, receiver, args);
        t.map(|t| t.safe(start))
    }

    /// The argument of a sink that the module's Python takes.
    fn param(&self, row: SinkRow) -> Arg {
        match row.python2 {
            Some(arg) if python2::runs_on(self.series) => arg,
            _ => row.arg,
        }
    }

    /// Whether a call of `path` that `row` matches is the sink: its guard is met, it calls no
    /// module an import from `odoo` binds when the row names the standard library's, and it sets
    /// no header of an outgoing client.
    fn is_sink(&self, row: SinkRow, path: &[String], call: &ast::ExprCall) -> bool {
        let odoo = row.stdlib && path.first().is_some_and(|p| self.odoo.contains(&p.as_str()));
        let client = matches!(&*call.func, Expr::Attribute(a)
            if headers_of(&a.value).is_some_and(|owner| self.is_client(owner)));
        !odoo && !client && row.guard.is_none_or(|g| self.met(g, call))
    }

    /// Whether the argument a guard reads passes its test, or the module's series predates the
    /// parameter.
    fn met(&self, guard: Guard, call: &ast::ExprCall) -> bool {
        if guard.since.is_some_and(|since| self.series.is_some_and(|s| s < since)) {
            return true;
        }
        let arg = match call.arguments.find_keyword(guard.keyword) {
            Some(k) => Some(&k.value),
            None => call.arguments.args.get(guard.pos),
        };
        match guard.test {
            Test::Is(value) => matches!(arg, Some(Expr::BooleanLiteral(b)) if b.value == value),
            Test::Unless(names) => arg.and_then(|a| self.named(a)).is_none_or(|n| !names.contains(&n)),
            Test::Resolves(until) => self.resolves(arg, until),
        }
    }

    /// Whether the parser passed as `arg`, or the default one without it, resolves entities.
    fn resolves(&self, arg: Option<&Expr>, until: Series) -> bool {
        match arg.map_or(Entities::Default, |a| self.entities(a)) {
            Entities::Resolved => true,
            Entities::Default => self.series.is_some_and(|s| s < until),
            Entities::Kept => false,
        }
    }

    /// What the parser `expr` names does with entities: `None`, and `XMLParser()` inline or bound
    /// to a local, count; any other parser resolves none.
    fn entities(&self, expr: &Expr) -> Entities {
        match expr {
            Expr::NoneLiteral(_) => Entities::Default,
            Expr::Name(n) => self.state.parsers.get(n.id.as_str()).copied().unwrap_or(Entities::Kept),
            e => xml_parser(e).unwrap_or(Entities::Kept),
        }
    }

    /// Keeps a parameter whose keys the user may pick that `write()` or `create()` receives on the
    /// records of a model method, for the callers that call the method under `sudo()`. A public
    /// method's parameter is one too: an RPC call runs as the user, but a Python caller may call it
    /// under `sudo()`.
    fn self_write(&mut self, call: &ast::ExprCall, args: &[Option<Taint>], keywords: &[Option<Taint>]) {
        let Expr::Attribute(a) = &*call.func else { return };
        if !matches!(self.owner, Some(Receiver::Model(_))) || !matches!(a.attr.as_str(), "write" | "create") {
            return;
        }
        let records = self.path(&a.value);
        if runs_as(&records) != RunsAs::AsCaller {
            return;
        }
        let Some(t) = sink_arg(Arg::First, "OD704", call, args, keywords) else {
            return;
        };
        let step = Spanned {
            value: format!("{}.{}()", records.join("."), a.attr),
            pos: self.loc.at(call),
        };
        for mut t in t.split() {
            if t.kind == Kind::Keys && t.origin.param().is_some() {
                t.steps.push(step.clone());
                self.edge(t, FlowTarget::SelfWrite);
            }
        }
    }

    /// What a call no row names returns: string methods keep the taint of their receiver and
    /// arguments, and a module's function or a pattern's `match()` that of its arguments. Methods
    /// of other objects, such as recordsets, return values the user does not control.
    fn result(
        &mut self,
        call: &ast::ExprCall,
        path: &[String],
        receiver: Option<Taint>,
        args: Option<Taint>,
    ) -> Option<Taint> {
        let method = path.last().map(String::as_str).unwrap_or_default();
        let Expr::Attribute(a) = &*call.func else {
            let keeps = method == "dict";
            return derived(if keeps { args } else { values(args) });
        };
        if method == "update" {
            self.hold(&a.value, arguments(call));
            self.add_to(&a.value, args);
            return None;
        }
        // What a method returned is records as often as values: a call on it keeps what the
        // summary of that method may tell, and takes nothing from its own arguments.
        if receiver
            .as_ref()
            .is_some_and(|t| matches!(t.origin, FlowOrigin::Return(_)))
        {
            return receiver;
        }
        if let Some(t) = receiver {
            let copy = method == "copy";
            let t = t.each(|t| {
                t.upload = picks(t, method, call);
                if !copy {
                    t.kind = Kind::Value;
                }
                t.own = false;
            });
            return join(Some(t), values(args));
        }
        // A compiled pattern's match holds the groups of the string it matched. `search` is
        // left out, as on records it returns records rather than parts of the domain.
        let matched = matches!(method, "match" | "fullmatch");
        let plain = matches!(&*a.value, Expr::StringLiteral(_)) || self.is_module(&a.value);
        if plain || matched { derived(args) } else { None }
    }

    /// A call of a model's method that a summary may describe; `sudo()` and the like return the
    /// same records.
    fn method_target(&self, call: &ast::ExprCall) -> bool {
        matches!(&*call.func, Expr::Attribute(a)
            if !SAME_MODEL.contains(&a.attr.as_str()) && self.receiver(&a.value).is_some())
    }

    /// A dotted name rooted in a name the function never binds, such as `json` or `tools.misc`.
    fn is_module(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Name(n) => !matches!(n.id.as_str(), "self" | "cls" | "env") && !self.locals.contains(n.id.as_str()),
            // `request.env` and its models return records, not values derived from the arguments.
            Expr::Attribute(a) => a.attr.as_str() != "env" && self.is_module(&a.value),
            _ => false,
        }
    }

    /// Reports a value from a source at a sink, and keeps one from a parameter or a call for the
    /// callers' and callees' flows.
    fn sink(&mut self, sink: Sink, callee: String, t: Option<Taint>, expr: &Expr) {
        let pos = self.loc.at(expr);
        self.sink_pos(sink, callee, t, pos);
    }

    /// A value the user sets on entry is a finding, which names it alone. Any other value keeps an
    /// edge for each of its origins the sink may report, but for OD716, whose records only a route
    /// returns.
    fn sink_pos(&mut self, sink: Sink, callee: String, t: Option<Taint>, pos: Pos) {
        let code = code(sink);
        let reported = |t: &Taint| !t.safe.has(code) && (sink != Sink::SudoWrite || t.kind == Kind::Keys);
        let Some(t) = t.filter(reported) else {
            return;
        };
        if t.origin.is_user() {
            return self.report(code, callee, &t, pos);
        }
        if sink == Sink::Record {
            return;
        }
        for mut t in t.split().into_iter().filter(reported) {
            t.steps.push(Spanned {
                value: callee.clone(),
                pos,
            });
            self.edge(t, FlowTarget::Sink(code));
        }
    }

    fn report(&mut self, code: &'static str, callee: String, t: &Taint, pos: Pos) {
        let origin = &t.steps[0].value;
        let names: Vec<&str> = t.steps[1..].iter().map(|s| s.value.as_str()).collect();
        let message = message(code, &callee, origin, &names);
        let mut flow: Vec<Step> = t.steps.iter().map(step).collect();
        flow.push(Step {
            path: None,
            pos,
            label: callee,
        });
        self.diags.push(RawDiag::new(code, pos, message).with_flow(flow));
    }
}

/// What an lxml parser does with entities, from the safest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Entities {
    /// Any other value, or another parser such as `HTMLParser()`.
    Kept,
    /// The default of the lxml the series runs.
    Default,
    /// `resolve_entities=True`.
    Resolved,
}

/// What `XMLParser(...)` does with entities, by its `resolve_entities` literal.
fn xml_parser(expr: &Expr) -> Option<Entities> {
    let Expr::Call(call) = expr else { return None };
    if tail_name(&call.func) != Some("XMLParser") {
        return None;
    }
    Some(
        match call.arguments.find_keyword("resolve_entities").map(|k| &k.value) {
            None => Entities::Default,
            Some(Expr::BooleanLiteral(b)) if b.value => Entities::Resolved,
            Some(_) => Entities::Kept,
        },
    )
}

/// The tests that each hold where `test` does: the operands of an `and`, or `test` itself.
fn conjuncts(test: &Expr) -> Vec<&Expr> {
    operands(test, ast::BoolOp::And)
}

/// The tests that each hold where `test` holds for `op`: the operands of an `and` for `in`, where
/// `test` is true, and of an `or` for `not in`, where it is false.
fn holding(test: &Expr, op: CmpOp) -> Vec<&Expr> {
    let each = if op == CmpOp::In {
        ast::BoolOp::And
    } else {
        ast::BoolOp::Or
    };
    operands(test, each)
}

/// The boolean checks that pass wherever `test` holds for `op`: a conjunct of `holding` that calls
/// one, for `in`, and one under `not` where its operand fails. A conjunct `holding` leaves an `or`
/// or an `and` is a choice of options, any of which may be the one that let the code through, as
/// `has_access() or consteq()` does: a check passing where its option holds counts.
fn passing(test: &Expr, op: CmpOp) -> Vec<&ast::ExprCall> {
    holding(test, op)
        .into_iter()
        .flat_map(|t| match t {
            Expr::Call(c) if op == CmpOp::In && is_boolean_check(c) => vec![c],
            Expr::UnaryOp(u) if u.op == ast::UnaryOp::Not => passing(&u.operand, op.negate()),
            Expr::BoolOp(b) => b.values.iter().flat_map(|v| passing(v, op)).collect(),
            _ => vec![],
        })
        .collect()
}

/// Whether `call` returns whether a check passes rather than raising when it fails: `consteq()`
/// and `compare_digest()` (18.0 `odoo/tools/misc.py:1675`), `has_access()` (18.0
/// `odoo/models.py:4459`) and `check_access_token()` (18.0 `addons/payment/utils.py:32-43`).
fn is_boolean_check(call: &ast::ExprCall) -> bool {
    matches!(
        tail_name(&call.func),
        Some("consteq" | "compare_digest" | "has_access" | "check_access_token")
    )
}

/// The operands of `test` under `op`, nested ones included, or `test` itself.
fn operands(test: &Expr, op: ast::BoolOp) -> Vec<&Expr> {
    match test {
        Expr::BoolOp(b) if b.op == op => b.values.iter().flat_map(|v| operands(v, op)).collect(),
        _ => vec![test],
    }
}

/// Methods that return some of the records they are called on.
const SAME_RECORDS: &[&str] = &["browse", "exists", "filtered"];

/// What a test may compare of records, beyond `SAME_MODEL` and `SAME_RECORDS`, without checking
/// who may see them.
const UNCHECKED: &[&str] = &["ensure_one", "ids", "get"];

impl Browsed {
    /// The records once bound to a local, which the flow names and a check of it vouches for.
    fn bound(mut self, step: Spanned<String>) -> Self {
        if !self.names.contains(&step.value) {
            self.names.push(step.value.clone());
        }
        self.id.step(&step);
        self
    }
}

/// What the method `method` of records `b` returns of them, called as `c`, where `flag` says that
/// `sudo(False)` keeps uid 1 in superuser mode, from `SUDO_FLAG`.
fn kept(b: Browsed, c: &ast::ExprCall, method: &str, flag: bool) -> Option<Browsed> {
    match method {
        "sudo" => {
            let root = flag && b.root;
            Some(Browsed {
                sudo: root || call_suffix(c) == "()",
                root: root || is_root(c),
                ..b
            })
        }
        "with_user" => {
            let root = superuser::switch(c).is_some();
            Some(Browsed { sudo: root, root, ..b })
        }
        m if SAME_MODEL.contains(&m) || SAME_RECORDS.contains(&m) || matches!(m, "read" | "mapped") => Some(b),
        _ => None,
    }
}

/// The parts of an id that each pick records: the items of a list, the iterables of a
/// comprehension and the values of an `if` expression.
fn id_parts(e: &Expr) -> Option<Vec<&Expr>> {
    match e {
        Expr::List(ast::ExprList { elts, .. })
        | Expr::Tuple(ast::ExprTuple { elts, .. })
        | Expr::Set(ast::ExprSet { elts, .. }) => Some(elts.iter().collect()),
        Expr::ListComp(ast::ExprListComp { generators, .. })
        | Expr::SetComp(ast::ExprSetComp { generators, .. })
        | Expr::Generator(ast::ExprGenerator { generators, .. }) => Some(generators.iter().map(|g| &g.iter).collect()),
        Expr::If(i) => Some(vec![&*i.body, &*i.orelse]),
        _ => None,
    }
}

/// Whether `expr` is `int(x)` or `float(x)`, or an `if` expression one of whose values is.
fn is_cast(expr: &Expr) -> bool {
    match expr {
        Expr::Call(c) => matches!(&*c.func, Expr::Name(n) if matches!(n.id.as_str(), "int" | "float")),
        Expr::If(i) => is_cast(&i.body) || is_cast(&i.orelse),
        _ => false,
    }
}

/// The parts of a container or an expression whose value may be one of them.
fn parts(expr: &Expr) -> Vec<&Expr> {
    fn comprehension<'e>(elt: &'e Expr, generators: &'e [ast::Comprehension]) -> Vec<&'e Expr> {
        std::iter::once(elt).chain(generators.iter().map(|g| &g.iter)).collect()
    }
    match expr {
        Expr::List(ast::ExprList { elts, .. })
        | Expr::Tuple(ast::ExprTuple { elts, .. })
        | Expr::Set(ast::ExprSet { elts, .. }) => elts.iter().collect(),
        Expr::Dict(d) => d.items.iter().map(|i| &i.value).collect(),
        Expr::BoolOp(b) => b.values.iter().collect(),
        Expr::If(i) => vec![&*i.body, &*i.orelse],
        Expr::Starred(ast::ExprStarred { value, .. }) | Expr::Await(ast::ExprAwait { value, .. }) => vec![&**value],
        Expr::ListComp(ast::ExprListComp { elt, generators, .. })
        | Expr::SetComp(ast::ExprSetComp { elt, generators, .. })
        | Expr::Generator(ast::ExprGenerator { elt, generators, .. }) => comprehension(elt, generators),
        Expr::DictComp(d) => comprehension(&d.value, &d.generators),
        _ => vec![],
    }
}

/// The names an expression reads.
#[derive(Default)]
struct Names(Vec<String>);

impl<'a> Visitor<'a> for Names {
    fn visit_expr(&mut self, expr: &'a Expr) {
        if let Expr::Name(n) = expr
            && !self.0.iter().any(|m| m == n.id.as_str())
        {
            self.0.push(n.id.to_string());
        }
        visitor::walk_expr(self, expr);
    }
}

/// Whether an expression reads a source, or a local or attribute of `self` whose taint a header
/// reports, other than through a sanitizer, as `Flows::expr` would find.
struct Reads<'f, 'a> {
    flows: &'f Flows<'a>,
    found: bool,
}

impl<'e> Visitor<'e> for Reads<'_, '_> {
    fn visit_expr(&mut self, expr: &'e Expr) {
        if self.found {
            return;
        }
        let flows = self.flows;
        let role = match expr {
            Expr::Attribute(_) => flows.table.lookup(&flows.path(expr)).map(|(_, role)| role),
            Expr::Call(c) => flows.table.lookup(&flows.path(&c.func)).map(|(_, role)| role),
            _ => None,
        };
        let key = match expr {
            Expr::Name(n) => Some(n.id.to_string()),
            e => self_attr(e),
        };
        let held = key
            .and_then(|k| flows.state.vars.get(&k))
            .map(|t| !t.safe.has(code(Sink::Header)));
        match (role, held) {
            (Some(Role::Source(_)), _) => self.found = true,
            (Some(Role::Sanitizer), _) => {}
            (Some(Role::Clears(cleared)), _) if cleared == code(Sink::Header) => {}
            (_, Some(found)) => self.found = found,
            _ => visitor::walk_expr(self, expr),
        }
    }
}

/// The locals a test of `Flows::tested` vouches for.
struct Tests<'f, 'a> {
    flows: &'f Flows<'a>,
    op: CmpOp,
    names: Vec<String>,
}

impl Tests<'_, '_> {
    /// The local of the held records `expr` is, or reads data of, as `order.partner_id` or
    /// `group._access_token()` do; `order.sudo().exists()`, `values.get("x")` and `ids` read none.
    fn held<'e>(&self, expr: &'e Expr) -> Option<&'e str> {
        let (mut e, mut reads) = (expr, Expr::is_name_expr(expr));
        loop {
            match e {
                Expr::Attribute(a) => {
                    let method = a.attr.as_str();
                    reads |= !(UNCHECKED.contains(&method)
                        || SAME_MODEL.contains(&method)
                        || SAME_RECORDS.contains(&method));
                    e = &a.value;
                }
                Expr::Call(c) => e = &c.func,
                _ => break,
            }
        }
        let name = e.as_name_expr()?.id.as_str();
        (reads && self.flows.state.browsed.contains_key(name)).then_some(name)
    }

    /// Records the test compares or compares a field of, and the id it holds to values the code
    /// allows.
    fn compared(&mut self, operands: &[Expr], op: CmpOp) {
        let held: Vec<String> = operands
            .iter()
            .filter_map(|o| self.held(o))
            .map(str::to_owned)
            .collect();
        self.names.extend(held);
        let id = match &operands[0] {
            Expr::Call(call) if is_cast(&operands[0]) => call.arguments.args.first(),
            e => Some(e),
        };
        let allowed = !tail_name(&operands[1]).is_some_and(|n| EXISTING.contains(&n));
        if let Some(id) = id.and_then(Expr::as_name_expr).filter(|_| allowed && op == self.op) {
            self.names.push(id.id.to_string());
        }
    }
}

impl<'a> Visitor<'a> for Tests<'_, '_> {
    /// A boolean check vouches only as `passing` reads it.
    fn visit_expr(&mut self, expr: &'a Expr) {
        if let Expr::Call(c) = expr
            && !is_boolean_check(c)
        {
            self.names.extend(self.flows.checked_by(c));
        }
        if let Some((operands, op)) = comparison(expr) {
            self.compared(operands, op);
        }
        walk_compared(self, expr);
    }
}

/// The comparison under `not` that reads as its negation, as `not x in y` reads as `x not in y`.
fn negated(expr: &Expr) -> Option<&ast::ExprCompare> {
    let Expr::UnaryOp(u) = expr else { return None };
    let c = u.operand.as_compare_expr().filter(|_| u.op == ast::UnaryOp::Not)?;
    matches!(c.ops[..], [CmpOp::In | CmpOp::NotIn]).then_some(c)
}

/// The operands of a comparison with one operator, and that operator.
fn comparison(expr: &Expr) -> Option<(&[Expr], CmpOp)> {
    match expr {
        Expr::Compare(c) if c.ops.len() == 1 => Some((&c.operands, c.ops[0])),
        e => negated(e).map(|c| (&c.operands[..], c.ops[0].negate())),
    }
}

/// Walks `expr`, but the comparison under a `not` that `negated` reads, which holds for the
/// negated operator only.
fn walk_compared<'a, V: Visitor<'a>>(visitor: &mut V, expr: &'a Expr) {
    match negated(expr) {
        Some(c) => c.operands.iter().for_each(|o| visitor.visit_expr(o)),
        None => visitor::walk_expr(visitor, expr),
    }
}

/// How a call ends its name in a dotted path: a `sudo()` whose argument is neither absent, `True`
/// nor the superuser reads as `sudo(False)`, since a flag the code computes may leave superuser
/// mode, and before saas~15.3 any other argument is a user.
fn call_suffix(c: &ast::ExprCall) -> &'static str {
    let superuser = |a: &Expr| is_true(a) || superuser::is_superuser(a);
    if tail_name(&c.func) == Some("sudo") && !sudo_argument(c).is_none_or(superuser) {
        "(False)"
    } else {
        "()"
    }
}

fn is_true(expr: &Expr) -> bool {
    matches!(expr, Expr::BooleanLiteral(b) if b.value)
}

fn toggle(set: &mut FxHashSet<String>, name: &str, on: bool) {
    if on {
        set.insert(name.to_owned());
    } else {
        set.remove(name);
    }
}

/// The methods that read, write or search the records they are called on.
fn is_data_method(name: &str) -> bool {
    name.starts_with("search")
        || matches!(
            name,
            "read_group" | "_read_group" | "formatted_read_group" | "read" | "mapped" | "create" | "write" | "unlink"
        )
}

/// What a call of a row that neither enters, ends nor reports a flow returns: what `Clears`,
/// `Keeps` or `Leaf` keep of its arguments.
fn transformed(
    role: Role,
    call: &ast::ExprCall,
    args: Vec<Option<Taint>>,
    keywords: Vec<Option<Taint>>,
) -> Option<Taint> {
    let t = match role {
        Role::Clears(code) => {
            let t = args.into_iter().chain(keywords).fold(None, join);
            values(t).map(|t| t.safe(Safe::of(&[code])))
        }
        Role::Keeps(positions, names) => match getattr_name(call) {
            Some(name) => {
                let mut args = args.into_iter();
                let object = attribute(args.next().flatten(), name);
                join(object, args.nth(1).flatten())
            }
            None => keeps(positions, names, call, args, keywords),
        },
        Role::Leaf => {
            let mut args = args;
            hold_value(&call.arguments.args, false, &mut args);
            values(args.into_iter().chain(keywords).fold(None, join))
        }
        Role::Source(_) | Role::Sink(_) | Role::Sanitizer => None,
    };
    derived(t)
}

/// What the positional arguments at `positions`, and the keywords `names`, carry.
fn keeps(
    positions: &[usize],
    names: &[&str],
    call: &ast::ExprCall,
    args: Vec<Option<Taint>>,
    keywords: Vec<Option<Taint>>,
) -> Option<Taint> {
    let kept = args.into_iter().enumerate().filter(|(i, _)| positions.contains(i));
    let named = call
        .arguments
        .keywords
        .iter()
        .zip(keywords)
        .filter(|(k, _)| k.arg.as_ref().is_some_and(|a| names.contains(&a.as_str())));
    let t = kept.map(|(_, t)| t).chain(named.map(|(_, t)| t)).fold(None, join);
    values(t)
}

/// The literal name `getattr()` reads, which reads as that attribute.
fn getattr_name(call: &ast::ExprCall) -> Option<&str> {
    let name = call.arguments.args.get(1).and_then(str_lit);
    name.filter(|_| tail_name(&call.func) == Some("getattr"))
}

/// What a call returns of values of taint `t`: no file of the uploads, and a value derived from
/// them rather than their own, so that the `name` of a tar member is no picked file's field.
fn derived(t: Option<Taint>) -> Option<Taint> {
    t.map(|t| {
        t.each(|t| {
            t.upload = None;
            t.own = false;
        })
    })
}

/// The files `method` of a value of taint `t` returns, called as `call`: `get()` and `getlist()`
/// of the uploads pick those of a field the code names, and `values()` keeps them all.
fn picks(t: &Taint, method: &str, call: &ast::ExprCall) -> Option<Upload> {
    let field = call.arguments.args.first().and_then(str_lit);
    let picked = matches!(method, "get" | "getlist") && field.is_some();
    match t.upload? {
        Upload::Mapping if picked => Some(Upload::Picked),
        Upload::Mapping => (method == "values").then_some(Upload::Mapping),
        Upload::Picked => None,
    }
}

/// Marks the value of a domain's condition of the elements `elts`, of taints `ts`, harmless for
/// OD715: it filters on the condition's field only, unless the operator takes a domain. The field
/// and the operator keep their taint, so the condition is harmless only where the code picks both.
/// `list` when the elements are a list, whose name operator counts only after a written field.
fn hold_value(elts: &[Expr], list: bool, ts: &mut [Option<Taint>]) {
    if let (true, Some(Some(t))) = (is_condition(elts, list), ts.get_mut(2)) {
        t.each_mut(|t| t.safe = t.safe.union(Safe::of(&["OD715"])));
    }
}

/// The operators of a condition whose value filters the condition's field only, from 18.0
/// `odoo/osv/expression.py:140` and 20.0 `odoo/orm/domains.py:80`; `any` and its kin take a
/// domain.
const VALUE_OPERATORS: &[&str] = &[
    "=",
    "!=",
    "<",
    ">",
    "<=",
    ">=",
    "=?",
    "like",
    "not like",
    "ilike",
    "not ilike",
    "=like",
    "not =like",
    "=ilike",
    "not =ilike",
    "in",
    "not in",
    "child_of",
    "parent_of",
];

/// Whether three elements read as a condition rather than as three conditions, three domains or
/// three fields: an operator, or a name that may hold one, after a field that opens no domain.
/// In a `list`, a name holds an operator only after a field written as text, as `[d1, d2, t]`
/// lists three domains; a tuple or `Domain()` of three is a condition whatever its field.
fn is_condition(elts: &[Expr], list: bool) -> bool {
    let [field, op, _] = elts else { return false };
    let opens =
        matches!(field, Expr::List(_) | Expr::Tuple(_)) || str_lit(field).is_some_and(|f| matches!(f, "|" | "&" | "!"));
    let operator = match op {
        Expr::Name(_) => !list || matches!(field, Expr::StringLiteral(_) | Expr::FString(_)),
        _ => str_lit(op).is_some_and(|o| VALUE_OPERATORS.contains(&o)),
    };
    !opens && operator
}

/// Whether `expr` is written as a domain: a `Domain` call or attribute, or `~`, `&` or `|` of one.
/// `Domain.__and__` takes only a `Domain` (20.0 `odoo/orm/domains.py:317`), so one operand written
/// as a domain proves both are.
fn is_domain(expr: &Expr) -> bool {
    let of_domain = |e: &Expr| matches!(e, Expr::Attribute(a) if tail_name(&a.value) == Some("Domain"));
    match expr {
        Expr::Call(c) => tail_name(&c.func) == Some("Domain") || of_domain(&c.func),
        Expr::Attribute(_) => of_domain(expr),
        Expr::UnaryOp(u) => u.op == ast::UnaryOp::Invert && is_domain(&u.operand),
        Expr::BinOp(b) => {
            matches!(b.op, Operator::BitAnd | Operator::BitOr) && (is_domain(&b.left) || is_domain(&b.right))
        }
        _ => false,
    }
}

/// Whether `func` joins domains: `AND` or `OR` of `expression` or `Domain`, or a name in `joins`
/// the function does not rebind, as `from odoo.osv.expression import AND` binds it.
fn is_domain_join(func: &Expr, joins: &[&str], locals: &FxHashSet<String>) -> bool {
    match func {
        Expr::Attribute(a) => {
            matches!(a.attr.as_str(), "AND" | "OR")
                && tail_name(&a.value).is_some_and(|n| matches!(n, "expression" | "Domain"))
        }
        Expr::Name(n) => joins.contains(&n.id.as_str()) && !locals.contains(n.id.as_str()),
        _ => false,
    }
}

/// The list or tuple of field names a `search_read()` or `search_fetch()` call reads, which holds
/// names, never a condition.
fn field_names(call: &ast::ExprCall) -> Option<&Expr> {
    let reads = tail_name(&call.func).is_some_and(|n| matches!(n, "search_read" | "search_fetch"));
    let fields = arg_expr(FIELD_NAMES, call).filter(|e| matches!(e, Expr::List(_) | Expr::Tuple(_)));
    fields.filter(|_| reads)
}

/// Whose rights a method called on the records of the dotted path `records` runs with. A
/// `with_user()` ends the caller's environment, and `sudo(False)` clears `su` whatever the caller's
/// is (18.0 `odoo/models.py:6331`).
fn runs_as(records: &[String]) -> RunsAs {
    let kept = |p: &String| p.ends_with("()") && p != "with_user()";
    let on_self = records.first().is_some_and(|p| p == "self") && records[1..].iter().all(kept);
    if runs_as_superuser(records) {
        RunsAs::Superuser
    } else if on_self {
        RunsAs::AsCaller
    } else {
        RunsAs::User
    }
}

/// Whether the dotted path `records`, as `Flows::path` writes it, runs as the superuser.
fn runs_as_superuser(records: &[String]) -> bool {
    records.iter().any(|p| p == "sudo()")
}

/// The segment `Flows::path` puts before the `sudo()` of a call building the environment of uid
/// 1, which Odoo keeps in superuser mode whatever `su` is (18.0 `odoo/api.py:571-572`, 20.0
/// `odoo/orm/environments.py:66-67`). No pattern names it, as it is no identifier.
const UID_1: &str = "(uid 1)";

/// The first series whose `sudo()` takes a flag (saas~12.5 `odoo/models.py`). Before it,
/// `sudo(False)` calls `env(user=False)`, which runs as uid 0, so even uid 1 leaves superuser mode
/// (12.0 `odoo/api.py:859`, `odoo/models.py:4818`).
const SUDO_FLAG: Series = Series { major: 12, minor: 5 };

/// Drops from `path` the superuser mode a call leaves: `with_user()` for another user, when `user`,
/// drops each `sudo()` and `UID_1`, and `sudo(False)` each `sudo()` but those of uid 1. Callers
/// pass `user` for a `sudo(False)` before `SUDO_FLAG` too, as it then runs as uid 0.
fn leave(path: &mut Vec<String>, user: bool) {
    let mut after_root = false;
    path.retain(|p| {
        let root = std::mem::replace(&mut after_root, p == UID_1);
        match p.as_str() {
            "sudo()" => !user && root,
            UID_1 => !user,
            _ => true,
        }
    });
}

/// Whether a call builds the environment of uid 1: a switch to `SUPERUSER_ID`, or a `sudo()` given
/// the superuser as its user, as `sudo(user)` reads it before saas~15.3.
fn is_root(c: &ast::ExprCall) -> bool {
    superuser::root(c) || tail_name(&c.func) == Some("sudo") && sudo_argument(c).is_some_and(superuser::is_superuser)
}

/// The flag of a `sudo()`, or before saas~15.3 its user: by position, or by name, `user` up to 12.0
/// (12.0 `odoo/models.py:4789`; 15.0 `:5286-5288`).
fn sudo_argument(c: &ast::ExprCall) -> Option<&Expr> {
    let args = &c.arguments;
    args.find_argument_value("flag", 0)
        .or_else(|| args.find_keyword("user").map(|k| &k.value))
}

/// The call on the chain of `records` that makes them superuser, as the code writes it: the
/// nearest `sudo()` or switch, or `sudo()` for a local bound under one.
fn superuser_call(records: &Expr) -> String {
    let inner = match records {
        Expr::Call(c) if is_root(c) || superuser::switch(c).is_some() => {
            return format!("{}()", tail_name(&c.func).unwrap_or("sudo"));
        }
        Expr::Call(c) if tail_name(&c.func) == Some("sudo") && call_suffix(c) == "()" => None,
        Expr::Call(c) => Some(&*c.func),
        Expr::Attribute(a) => Some(&*a.value),
        Expr::Subscript(s) => Some(&*s.value),
        _ => None,
    };
    inner.map_or_else(|| "sudo()".to_owned(), superuser_call)
}

/// How a finding names the call of a sink row's `pattern`. `sudo().write` also matches what a
/// switch makes superuser, so a `sudo().` pattern names the call that does.
fn sink_callee(pattern: &str, call: &ast::ExprCall) -> String {
    match (pattern.strip_prefix("sudo()."), &*call.func) {
        (Some(m), Expr::Attribute(a)) if !m.contains('.') => format!("{}.{m}()", superuser_call(&a.value)),
        _ => format!("{pattern}()"),
    }
}

/// Whether `expr` is an environment: `env`, or a call of `env` or `Environment` that builds one.
fn is_env(expr: &Expr) -> bool {
    match expr {
        Expr::Call(c) => matches!(tail_name(&c.func), Some("env" | "Environment")),
        e => tail_name(e) == Some("env"),
    }
}

/// What `expr` is the `headers` of: a response, or an outgoing client as `Flows::is_client`
/// tells them apart.
fn headers_of(expr: &Expr) -> Option<&Expr> {
    match expr {
        Expr::Attribute(a) if a.attr.as_str() == "headers" => Some(&a.value),
        _ => None,
    }
}

/// The constructors of an outgoing HTTP client.
const CLIENTS: &[&str] = &[
    "requests.Session",
    "requests.session",
    "httpx.Client",
    "httpx.AsyncClient",
];

fn client_call(expr: &Expr) -> bool {
    matches!(expr, Expr::Call(c) if CLIENTS.contains(&dotted(&c.func).as_str()))
}

fn is_session(expr: &Expr) -> bool {
    let name = match expr {
        Expr::Call(c) => tail_name(&c.func),
        e => tail_name(e),
    };
    name.is_some_and(|n| n.to_ascii_lowercase().contains("session"))
}

/// The expression a call passes for `arg`, positional first, as `sink_arg` reads its taint.
fn arg_expr(arg: Arg, call: &ast::ExprCall) -> Option<&Expr> {
    let (pos, names) = match arg {
        Arg::First => (0, SINK_KEYWORDS),
        Arg::At(pos, names) => (pos, names),
        Arg::Any => (0, &[][..]),
        Arg::Each(each) => return each.first().and_then(|a| arg_expr(*a, call)),
    };
    match call.arguments.args.get(pos) {
        Some(e) => Some(e),
        None => names
            .iter()
            .find_map(|n| call.arguments.find_keyword(n))
            .map(|k| &k.value),
    }
}

/// The key and value of each entry of a list or tuple of pairs or of a mapping, as headers are
/// passed, or `None` when `expr` is neither or holds an entry it cannot split, such as a starred
/// item or a `**` spread, whose taint no named entry would account for.
fn header_entries(expr: &Expr) -> Option<Vec<(&Expr, &Expr)>> {
    fn pair(e: &Expr) -> Option<(&Expr, &Expr)> {
        match e {
            Expr::Tuple(t) if t.elts.len() == 2 => Some((&t.elts[0], &t.elts[1])),
            _ => None,
        }
    }
    match expr {
        Expr::List(ast::ExprList { elts, .. }) | Expr::Tuple(ast::ExprTuple { elts, .. }) => {
            elts.iter().map(pair).collect()
        }
        Expr::Dict(d) => d.items.iter().map(|i| Some((i.key.as_ref()?, &i.value))).collect(),
        _ => None,
    }
}

/// The step a header sink takes in a flow, naming the header when the code does.
fn header_label(name: Option<&str>, callee: &str) -> String {
    match name {
        Some(name) => format!("the {name} header of {callee}"),
        None => format!("a header of {callee}"),
    }
}

/// Whether `expr` is `self` or `cls`.
fn of_self(expr: &Expr) -> bool {
    matches!(expr, Expr::Name(n) if matches!(n.id.as_str(), "self" | "cls"))
}

/// The key `self.<attr>` keeps a value under, as a local's name does.
fn self_attr(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Attribute(a) if matches!(&*a.value, Expr::Name(n) if n.id.as_str() == "self") => {
            Some(format!("self.{}", a.attr))
        }
        _ => None,
    }
}

/// The key a local or an attribute of `self` keeps a value under.
fn var_key(expr: &Expr) -> Option<String> {
    self_attr(expr).or_else(|| expr.as_name_expr().map(|n| n.id.to_string()))
}

/// Whether every prefix `startswith()` takes, one or a tuple, is `fixed`.
fn fixed_prefixes(arg: &Expr, fixed: impl FnMut(&Expr) -> bool) -> bool {
    let prefixes: Vec<&Expr> = match arg {
        Expr::Tuple(t) => t.elts.iter().collect(),
        p => vec![p],
    };
    !prefixes.is_empty() && prefixes.into_iter().all(fixed)
}

/// The local whose start `expr` tests where it holds for `op`, `p.startswith(prefix)` for `in`
/// and `not p.startswith(prefix)` for `not in`, with what `startswith()` takes.
fn started(expr: &Expr, op: CmpOp) -> Option<(&str, &Expr)> {
    let call = match (op, expr) {
        (CmpOp::In, Expr::Call(call)) => call,
        (CmpOp::NotIn, Expr::UnaryOp(u)) if u.op == ast::UnaryOp::Not => u.operand.as_call_expr()?,
        _ => return None,
    };
    let Expr::Attribute(a) = &*call.func else { return None };
    let name = a.value.as_name_expr().filter(|_| a.attr.as_str() == "startswith")?;
    Some((name.id.as_str(), call.arguments.args.first()?))
}

/// The attributes the ORM sets on a model that `Flows::chosen` does not read as a collection the
/// code chose: a string, or what the records or the request hold.
const MODEL_ATTRS: &[&str] = &[
    "_fields",
    "_context",
    "_ids",
    "_origin",
    "_name",
    "_description",
    "_order",
    "_rec_name",
    "_table",
];

/// The containers whose membership proves only that a model or a field exists: `env`, the
/// registry or `pool`, and `_fields`.
const EXISTING: &[&str] = &["env", "registry", "pool", "_fields"];

/// The rules an existing model or field name is harmless for: those of a model, a field or a
/// domain stay open, as the user still picks which one.
fn existing() -> Safe {
    let open = ["OD712", "OD714", "OD715"];
    let codes: Vec<&str> = SINKS.iter().map(|(_, c)| *c).filter(|c| !open.contains(c)).collect();
    Safe::of(&codes)
}

/// Joins each entry of `other` into the one of `mine` under its name.
fn join_into<T>(
    mine: &mut FxHashMap<String, T>,
    other: FxHashMap<String, T>,
    join: fn(Option<T>, Option<T>) -> Option<T>,
) {
    for (name, t) in other {
        let kept = mine.remove(&name);
        if let Some(t) = join(kept, Some(t)) {
            mine.insert(name, t);
        }
    }
}

fn merge_into(joined: &mut Option<State>, state: State) {
    match joined {
        Some(j) => j.merge(state),
        None => *joined = Some(state),
    }
}

/// The names a membership test checks: `x <op> allowed` checks `x`, and
/// `any(k <op> allowed for k in vals)` checks the keys of `vals`, each with its container, or
/// `None` for keys tested against `EXISTING`. `starts` holds the names whose start
/// `x.startswith(p)` checks, `not` before it for `not in`, and `exists` those a test against a
/// container of `EXISTING`, as `x <op> self.env`, `self.pool` or `Model._fields`, checks.
struct Checks<'a> {
    op: CmpOp,
    values: Vec<(String, &'a Expr)>,
    keys: Vec<(String, Option<&'a Expr>)>,
    starts: Vec<String>,
    exists: Vec<String>,
}

impl<'a> Checks<'a> {
    fn new(op: CmpOp) -> Self {
        Self {
            op,
            values: vec![],
            keys: vec![],
            starts: vec![],
            exists: vec![],
        }
    }

    /// The name a membership test checks, its container, and whether that holds every model or
    /// field.
    fn checked(&self, expr: &'a Expr) -> Option<(String, &'a Expr, bool)> {
        let (operands, op) = comparison(expr)?;
        let name = operands[0].as_name_expr().filter(|_| op == self.op)?;
        let every = tail_name(&operands[1]).is_some_and(|n| EXISTING.contains(&n));
        Some((name.id.to_string(), &operands[1], every))
    }

    /// The name whose start a `startswith()` test holds to prefixes that keep it harmless.
    fn started(&self, expr: &Expr) -> Option<String> {
        let (name, prefix) = started(expr, self.op)?;
        fixed_prefixes(prefix, |p| str_lit(p).is_some_and(fixes_start)).then(|| name.to_owned())
    }

    fn comprehension(&mut self, elt: &'a Expr, generators: &'a [ast::Comprehension]) {
        let mut inner = Checks::new(self.op);
        inner.visit_expr(elt);
        for g in generators {
            let Some(target) = g.target.as_name_expr() else {
                continue;
            };
            let mapping = match &g.iter {
                Expr::Call(c) => match &*c.func {
                    Expr::Attribute(a) if a.attr.as_str() == "keys" => &*a.value,
                    _ => continue,
                },
                e => e,
            };
            // Keys tested against `_fields` end OD704 too: such a test shows the code means to
            // take any field the model has.
            let Some(m) = mapping.as_name_expr() else { continue };
            let name = target.id.as_str();
            if let Some((_, container)) = inner.values.iter().find(|(v, _)| v == name) {
                self.keys.push((m.id.to_string(), Some(container)));
            } else if inner.exists.iter().any(|v| v == name) {
                self.keys.push((m.id.to_string(), None));
            }
        }
    }
}

impl<'a> Visitor<'a> for Checks<'a> {
    fn visit_expr(&mut self, expr: &'a Expr) {
        match expr {
            Expr::ListComp(ast::ExprListComp { elt, generators, .. })
            | Expr::SetComp(ast::ExprSetComp { elt, generators, .. })
            | Expr::Generator(ast::ExprGenerator { elt, generators, .. }) => {
                return self.comprehension(elt, generators);
            }
            _ => {
                match self.checked(expr) {
                    Some((name, _, true)) => self.exists.push(name),
                    Some((name, container, false)) => self.values.push((name, container)),
                    None => {}
                }
                self.starts.extend(self.started(expr));
            }
        }
        walk_compared(self, expr);
    }
}

/// The value of a call that a sink of `code` checks. Of every positional argument, or of each
/// argument `Each` lists, the first not harmless for the code.
fn sink_arg(
    arg: Arg,
    code: &str,
    call: &ast::ExprCall,
    args: &[Option<Taint>],
    keywords: &[Option<Taint>],
) -> Option<Taint> {
    let (pos, names) = match arg {
        Arg::First => (0, SINK_KEYWORDS),
        Arg::At(pos, names) => (pos, names),
        Arg::Any => return args.iter().flatten().find(|t| !t.safe.has(code)).cloned(),
        Arg::Each(each) => {
            let open = |a: &Arg| sink_arg(*a, code, call, args, keywords).filter(|t| !t.safe.has(code));
            return each.iter().find_map(open);
        }
    };
    if call.arguments.args.len() > pos {
        return args.get(pos).cloned().flatten();
    }
    call.arguments
        .keywords
        .iter()
        .zip(keywords)
        .find(|(k, _)| k.arg.as_ref().is_some_and(|a| names.contains(&a.as_str())))
        .and_then(|(_, t)| t.clone())
}

/// Where a literal template's `.format()` starts: the text before its first field, or the value
/// of that field.
fn format_start(call: &ast::ExprCall, args: &[Option<Taint>], keywords: &[Option<Taint>]) -> Safe {
    let Expr::Attribute(a) = &*call.func else {
        return Safe::default();
    };
    let Some(template) = constant(&a.value).filter(|_| a.attr.as_str() == "format") else {
        return Safe::default();
    };
    let (prefix, rest) = template_prefix(&template, '{');
    if !prefix.is_empty() {
        return literal_start(&prefix);
    }
    let field = rest.split(['}', ':', '!', '.', '[']).next().unwrap_or_default();
    let value = match field {
        "" | "0" => call.arguments.args.first().zip(args.first()),
        name => call
            .arguments
            .keywords
            .iter()
            .zip(keywords)
            .find(|(k, _)| k.arg.as_ref().is_some_and(|a| a.as_str() == name))
            .map(|(k, t)| (&k.value, t)),
    };
    value.map_or_else(Safe::default, |(e, t)| start_of(e, t.as_ref()))
}

fn step(s: &Spanned<String>) -> Step {
    Step {
        path: None,
        pos: s.pos,
        label: s.value.clone(),
    }
}

fn code(sink: Sink) -> &'static str {
    let row = SINKS.iter().find(|(s, _)| *s == sink);
    row.expect("every sink has a row in SINKS").1
}

/// The message of a flow from `origin` to the call `callee`, naming the locals and methods it
/// went through, without lines, so that a baseline keeps it when lines move.
pub(crate) fn message(code: &str, callee: &str, origin: &str, names: &[&str]) -> String {
    let mut through: Vec<&str> = vec![];
    for name in names {
        if through.last() != Some(name) {
            through.push(name);
        }
    }
    let through = if through.is_empty() {
        String::new()
    } else {
        format!(" through '{}'", through.join("', '"))
    };
    let (lead, tail) = MESSAGES
        .iter()
        .find(|(c, ..)| *c == code)
        .map_or(("", ""), |(_, lead, tail)| (*lead, *tail));
    let tail = if code == "OD711" && callee.to_ascii_lowercase().contains("content-disposition") {
        DISPOSITION
    } else {
        tail
    };
    format!("{origin} reaches {lead}{callee}{through}{tail}")
}

/// What the message of each rule says before the callee, and after the locals the flow went
/// through.
const MESSAGES: &[(&str, &str, &str)] = &[
    (
        "OD701",
        "the query of ",
        "; pass it as a query parameter or build the query with odoo.tools.SQL",
    ),
    ("OD702", "", ", which runs it as code"),
    (
        "OD703",
        "",
        " unescaped; escape it, or pass it as an argument of Markup's % or format()",
    ),
    (
        "OD704",
        "",
        ", so the user picks the fields it sets as superuser; copy only the fields the user may set",
    ),
    (
        "OD705",
        "the URL of ",
        ", so the user picks the site the browser goes to; start the URL with a path the code picks",
    ),
    (
        "OD706",
        "the URL of ",
        ", so the user picks the host the server connects to; start the URL with a host the code picks",
    ),
    (
        "OD707",
        "the path of ",
        ", so the user picks the file the server opens; pass the name through secure_filename() or os.path.basename()",
    ),
    (
        "OD708",
        "the command of ",
        ", so the user picks what the shell runs; pass the arguments as a list without shell=True, or quote each with shlex.quote()",
    ),
    (
        "OD709",
        "",
        ", which trusts the data it loads, so the user may run code on the server; load it with json.loads() or yaml.safe_load()",
    ),
    (
        "OD710",
        "",
        ", whose parser resolves entities, so the user can read the server's files; pass etree.XMLParser(resolve_entities=False)",
    ),
    (
        "OD711",
        "",
        ", so the user picks how the browser reads the response; pick the value from those the code allows",
    ),
    (
        "OD712",
        "the model of ",
        ", so the user picks the model the superuser reads or writes; pick the model from those the code allows",
    ),
    (
        "OD713",
        "the name of ",
        ", whose result the code calls, so the user picks the method that runs; pick the name from those the code allows",
    ),
    (
        "OD714",
        "the field of ",
        ", so the user reads any field as superuser; pick the field from those the code allows",
    ),
    (
        "OD715",
        "the domain or the fields of ",
        ", so the user searches or groups by any field as superuser; build each condition with a field and an operator the code picks",
    ),
    (
        "OD716",
        "the id of ",
        ", whose records the route returns read as superuser; check them with _document_check_access(), consteq(), check_access() from 18.0 or check_access_rule() before 18.0",
    ),
    (
        "OD717",
        "the content of ",
        ", which public=True lets anyone download; keep it private and share it with generate_access_token()",
    ),
];

const DISPOSITION: &str =
    ", so the user picks how the browser reads the response; pass the file name through content_disposition()";

#[cfg(test)]
mod tests {
    use ruff_python_parser::parse_module;

    use super::*;

    /// The names of a file that imports nothing from `odoo` and binds no collection.
    const NAMES: FileNames<'static> = FileNames {
        odoo: &[],
        collections: &[],
        domain_joins: &[],
    };

    /// The findings of the first function in `src`, as `code message`.
    fn scan(params: Params, src: &str) -> Vec<String> {
        scan_with(&TaintTable::default(), params, src)
    }

    fn scan_with(table: &TaintTable, params: Params, src: &str) -> Vec<String> {
        scan_in(table, None, params, src)
    }

    /// The findings of the first function in `src`, in a module of `series`, after the imports
    /// before it.
    fn scan_in(table: &TaintTable, series: Option<&str>, params: Params, src: &str) -> Vec<String> {
        let series = series.and_then(Series::parse);
        let parsed = parse_module(src).expect("valid Python");
        let body = &parsed.syntax().body;
        let Some(func) = body.iter().find_map(Stmt::as_function_def_stmt) else {
            panic!("no function");
        };
        let top = crate::module_state::top_level(body);
        let imported = crate::module_state::odoo_names(&top);
        let odoo: Vec<&str> = imported.iter().map(|(local, _)| *local).collect();
        let domain_joins = domain_joins(&imported);
        let collections = crate::module_state::collections(&top);
        let names = FileNames {
            odoo: &odoo,
            collections: &collections,
            domain_joins: &domain_joins,
        };
        let loc = Locator::new(src);
        function(func, params, None, series, &names, &loc, table)
            .0
            .into_iter()
            .map(|d| format!("{} {}", d.code, d.message.split(';').next().unwrap_or_default()))
            .collect()
    }

    /// The codes of `found`, findings as `scan` writes them.
    fn codes_from(found: &[String]) -> Vec<String> {
        found.iter().map(|f| f[..5].to_owned()).collect()
    }

    #[test]
    fn follows_assignments_to_the_sink() {
        let src = "def f(self, term):\n    where = \"name = '%s'\" % term\n    q = 'SELECT 1 WHERE ' + where\n    self.env.cr.execute(q)\n";
        assert_eq!(
            scan(Params::Route(&[]), src),
            ["OD701 route parameter 'term' reaches the query of cr.execute() through 'where', 'q'"]
        );
        assert!(scan(Params::None, src).is_empty());
    }

    #[test]
    fn records_the_path_with_positions() {
        let src = "def f(self):\n    data = request.get_json_data()\n    safe_eval(data['x'])\n";
        let parsed = parse_module(src).unwrap();
        let Some(Stmt::FunctionDef(func)) = parsed.syntax().body.first() else {
            panic!()
        };
        let (diags, _) = function(
            func,
            Params::None,
            None,
            None,
            &NAMES,
            &Locator::new(src),
            &TaintTable::default(),
        );
        let flow: Vec<_> = diags[0].flow.iter().map(|s| (s.pos.line, s.label.as_str())).collect();
        assert_eq!(flow, [(2, "request.get_json_data()"), (2, "data"), (3, "safe_eval()")]);
    }

    #[test]
    fn joins_branches_and_skips_those_that_exit() {
        let joined = "def f(self, x):\n    q = 'a'\n    if self:\n        q = x\n    self.env.cr.execute(q)\n";
        assert_eq!(scan(Params::Route(&[]), joined).len(), 1);
        let exits =
            "def f(self, x):\n    q = 'a'\n    if self:\n        q = x\n        return\n    self.env.cr.execute(q)\n";
        assert!(scan(Params::Route(&[]), exits).is_empty());
        let both =
            "def f(self, x):\n    if self:\n        q = x\n    else:\n        q = 'a'\n    self.env.cr.execute(q)\n";
        assert_eq!(scan(Params::Route(&[]), both).len(), 1);
    }

    #[test]
    fn sudo_writes_need_user_chosen_keys() {
        let write = |body: &str| {
            scan(
                Params::Route(&[]),
                &format!("def f(self, name, **kw):\n    M = request.env['x'].sudo()\n{body}\n"),
            )
        };
        assert_eq!(write("    M.create(dict(kw))").len(), 1);
        assert_eq!(write("    M.create({**kw})").len(), 1);
        assert_eq!(write("    vals = {}\n    vals.update(kw)\n    M.create(vals)").len(), 1);
        assert_eq!(write("    M.browse(1).write(kw)").len(), 1);
        assert!(write("    M.create({'name': kw.get('name')})").is_empty());
        assert!(write("    M.create({'name': name})").is_empty());
        assert!(write("    M.create({k: kw[k] for k in kw if k in ('a',)})").is_empty());
        assert!(write("    request.env['x'].create(kw)").is_empty());
    }

    #[test]
    fn a_source_value_keeps_the_keys_of_an_rpc_mapping() {
        for body in [
            "    vals['note'] = request.params.get('note')\n    self.sudo().write(vals)",
            "    vals.update(note=request.params.get('note'))\n    self.sudo().write(vals)",
            "    self.sudo().write(dict(vals, note=request.params.get('note')))",
        ] {
            let src = format!("def save(self, vals):\n{body}\n");
            assert_eq!(scan(Params::Rpc, &src).len(), 1, "{body}");
        }
    }

    #[test]
    fn sanitizers_and_checks_end_the_flow() {
        let sql = |body: &str| scan(Params::Rpc, &format!("def f(self, x, vals):\n{body}\n"));
        assert!(sql("    self.env.cr.execute('SELECT %s' % int(x))").is_empty());
        assert!(sql("    self.env.cr.execute('SELECT %s', [x])").is_empty());
        assert!(sql("    self.env.cr.execute(SQL('SELECT %s', x))").is_empty());
        assert!(sql("    self.env.cr.execute(SQL('SELECT %s', SQL.identifier(x)))").is_empty());
        assert_eq!(sql("    self.env.cr.execute(SQL('SELECT ' + x))").len(), 1);
        assert!(sql("    assert x in ('a', 'b')\n    self.env.cr.execute('SELECT %s' % x)").is_empty());
        assert!(
            sql("    if x not in ('a', 'b'):\n        raise ValueError()\n    self.env.cr.execute('SELECT %s' % x)")
                .is_empty()
        );
        assert!(
            sql("    if any(k not in F for k in vals):\n        raise ValueError()\n    self.sudo().write(vals)")
                .is_empty()
        );
        assert!(sql("    self.sudo().write({k: 1 for k in set(vals) & set(F)})").is_empty());
        assert!(sql("    return Markup('<b>%s</b>') % x").is_empty());
        assert!(sql("    return escape(x)").is_empty());
    }

    #[test]
    fn records_and_recordsets_carry_no_taint() {
        let src = "def f(self, model, domain):\n    M = request.env.get(model)\n    q = M.sudo()._search(domain)\n    sql, params = q.select()\n    request.env.cr.execute(f'EXPLAIN {sql}', params)\n";
        assert!(scan(Params::Route(&[]), src).is_empty());
        let module = "def f(self, x):\n    self.env.cr.execute(json.dumps(x))\n";
        assert_eq!(scan(Params::Route(&[]), module).len(), 1);
    }

    #[test]
    fn markup_and_eval_sinks() {
        let src = "def f(self, **post):\n    a = Markup(post.get('a'))\n    b = Markup(f\"<p>{post['b']}</p>\")\n    return safe_eval(post['c'])\n";
        assert_eq!(
            scan(Params::Route(&[]), src),
            [
                "OD703 route parameter 'post' reaches Markup() unescaped",
                "OD703 route parameter 'post' reaches Markup() unescaped",
                "OD702 route parameter 'post' reaches safe_eval(), which runs it as code",
            ]
        );
    }

    /// The findings and the summary of the first function of `src`, a method of `x.y` in a module
    /// of no series.
    fn model_method(params: Params, src: &str) -> (Vec<RawDiag>, Option<FlowSummary>) {
        let parsed = parse_module(src).expect("valid Python");
        let Some(Stmt::FunctionDef(func)) = parsed.syntax().body.first() else {
            panic!("no function");
        };
        let owner = Some(Receiver::Model("x.y".to_owned()));
        function(
            func,
            params,
            owner,
            None,
            &NAMES,
            &Locator::new(src),
            &TaintTable::default(),
        )
    }

    fn summary(params: Params, src: &str) -> FlowSummary {
        model_method(params, src).1.expect("a summary")
    }

    #[test]
    fn summaries_keep_flows_from_parameters_and_calls() {
        let helper = summary(
            Params::None,
            "def _h(self, a, b):\n    q = 'SELECT %s' % b\n    self.env.cr.execute(q)\n    return a\n",
        );
        let targets: Vec<_> = helper.edges.iter().map(|e| (e.origin, e.target.clone())).collect();
        assert_eq!(
            targets,
            [
                (FlowOrigin::Param(1), FlowTarget::Sink("OD701")),
                (FlowOrigin::Param(0), FlowTarget::Return),
            ]
        );
        assert_eq!(helper.params, ["a", "b"]);
        let route = summary(
            Params::Route(&[]),
            "def f(self, term):\n    M = request.env['x.y'].sudo()\n    rows = M._h(1, b=term)\n    return Markup(self._g())\n",
        );
        let calls: Vec<_> = route
            .calls
            .iter()
            .map(|c| (c.receiver.clone(), c.method.as_str()))
            .collect();
        assert_eq!(
            calls,
            [
                (Receiver::Model("x.y".to_owned()), "_h"),
                (Receiver::Model("x.y".to_owned()), "_g"),
            ]
        );
        let targets: Vec<_> = route.edges.iter().map(|e| (e.origin, e.target.clone())).collect();
        assert_eq!(
            targets,
            [
                (FlowOrigin::Source, FlowTarget::Arg(0, ArgSlot::Name("b".to_owned()))),
                (FlowOrigin::Return(1), FlowTarget::Sink("OD703")),
            ]
        );
    }

    #[test]
    fn a_configured_source_reaches_a_built_in_sink() {
        let mut table = TaintTable::default();
        table.source("lib.read_form").unwrap();
        let src = "def f(self):\n    term = lib.read_form()\n    self.env.cr.execute('SELECT %s' % term)\n";
        assert_eq!(
            scan_with(&table, Params::None, src),
            ["OD701 lib.read_form() reaches the query of cr.execute() through 'term'"]
        );
    }

    #[test]
    fn a_configured_sink_receives_a_route_parameter() {
        let mut table = TaintTable::default();
        table.sink("OD702", "tools.run_code").unwrap();
        let src = "def f(self, code):\n    tools.run_code(code)\n";
        assert_eq!(
            scan_with(&table, Params::Route(&[]), src),
            ["OD702 route parameter 'code' reaches tools.run_code(), which runs it as code"]
        );
    }

    #[test]
    fn a_configured_sanitizer_ends_the_flow() {
        let mut table = TaintTable::default();
        table.sanitizer("lib.clean_term").unwrap();
        let src = "def f(self, x):\n    self.env.cr.execute('SELECT %s' % lib.clean_term(x))\n";
        assert_eq!(scan(Params::Rpc, src).len(), 1);
        assert!(scan_with(&table, Params::Rpc, src).is_empty());
    }

    #[test]
    fn a_configured_sanitizer_matches_a_call_only() {
        let mut table = TaintTable::default();
        table.sanitizer("record.safe_text").unwrap();
        let query = |text: &str| {
            let src = format!("def f(self, record):\n    self.env.cr.execute('SELECT %s' % {text})\n");
            scan_with(&table, Params::Route(&[]), &src)
        };
        assert_eq!(query("record.safe_text").len(), 1);
        assert!(query("record.safe_text()").is_empty());
    }

    #[test]
    fn a_last_item_matches_a_call_of_an_item_only() {
        let mut table = TaintTable::default();
        table.sink("OD702", "handlers.[]").unwrap();
        let src = "def f(self, k, x):\n    handlers[k](x)\n";
        assert_eq!(
            scan_with(&table, Params::Route(&[]), src),
            ["OD702 route parameter 'x' reaches handlers.[](), which runs it as code"]
        );
        let mut table = TaintTable::default();
        table.source("forms.data.[]").unwrap();
        table.sink("OD702", "sandbox.run").unwrap();
        let run = |arg: &str| scan_with(&table, Params::Rpc, &format!("def f(self):\n    sandbox.run({arg})\n"));
        assert!(run("forms.data['code']").is_empty());
        assert_eq!(run("forms.data[k]()").len(), 1);
    }

    #[test]
    fn a_configured_row_wins_over_a_built_in_one() {
        let mut table = TaintTable::default();
        table.sanitizer("sandbox.safe_eval").unwrap();
        let eval = |call: &str| scan_with(&table, Params::Rpc, &format!("def f(self, x):\n    {call}(x)\n"));
        assert!(eval("sandbox.safe_eval").is_empty());
        assert_eq!(
            eval("safe_eval"),
            ["OD702 RPC argument 'x' reaches safe_eval(), which runs it as code"]
        );
    }

    #[test]
    fn a_configured_markup_sink_escapes_nothing() {
        let mut table = TaintTable::default();
        table.sink("markup-injection-flow", "raw").unwrap();
        let src = "def f(self, x):\n    return Markup(raw('%s') % x)\n";
        assert_eq!(
            scan_with(&table, Params::Rpc, src),
            ["OD703 RPC argument 'x' reaches Markup() unescaped"]
        );
    }

    #[test]
    fn a_configured_sudo_segment_matches_what_runs_as_superuser() {
        let mut table = TaintTable::default();
        table.sink("OD702", "sudo().run").unwrap();
        table.sink("OD702", "sudo().with_user().launch").unwrap();
        let run = |call: &str| {
            scan_with(
                &table,
                Params::Rpc,
                &format!("def f(self, x):\n    self.env['a.b'].{call}(x)\n"),
            )
        };
        for call in [
            "sudo().run",
            "with_user(1).run",
            "with_user(SUPERUSER_ID).sudo(False).run",
            "sudo().with_user(SUPERUSER_ID).launch",
        ] {
            assert_eq!(codes_from(&run(call)), ["OD702"], "{call}");
        }
        for call in [
            "run",
            "sudo(False).run",
            "sudo().with_user(self.env.user).run",
            "sudo().with_user(self.env.user).launch",
        ] {
            assert!(run(call).is_empty(), "{call}");
        }
    }

    #[test]
    fn configured_rows_take_patterns_and_taint_rules_only() {
        let mut table = TaintTable::default();
        for pattern in ["", "a..b", "a.b()", "1a", "a.", "a.[]()", "a-b"] {
            assert!(table.source(pattern).is_err(), "{pattern}");
        }
        for pattern in [
            "cr.execute",
            "sudo().write",
            "env.[].create",
            "_x9",
            "handlers.[]",
            "[]",
        ] {
            assert!(table.source(pattern).is_ok(), "{pattern}");
        }
        let error = table.source("a.b()").unwrap_err();
        assert!(error.starts_with("'a.b()' is not a taint pattern"), "{error}");
        assert!(error.contains("never an item read"), "{error}");
        let error = table.sink("OD101", "lib.run").unwrap_err();
        assert!(error.contains("'OD101'"), "{error}");
        assert!(table.sink("no-such-rule", "lib.run").is_err());
        assert!(table.sink("sql-injection-flow", "lib.run").is_ok());
        assert!(table.sink("od704", "lib.run").is_ok());
        let error = table.sink("sudo-record-exposure", "lib.show").unwrap_err();
        assert!(error.contains("OD701 to OD715"), "{error}");
        assert!(table.sink("OD717", "lib.store").is_err());
        assert!(table.sink("sudo-public-attachment", "lib.store").is_err());
    }

    #[test]
    fn attributes_of_self_hold_values_like_locals() {
        let query = |attr: &str| {
            scan(
                Params::Route(&[]),
                &format!(
                    "def f(self, term):\n    self.term = term\n    self.env.cr.execute('SELECT %s' % self.{attr})\n"
                ),
            )
        };
        assert_eq!(
            query("term"),
            ["OD701 route parameter 'term' reaches the query of cr.execute() through 'self.term'"]
        );
        assert!(query("other").is_empty());
        let src =
            "def f(self, **kw):\n    self.vals = {}\n    self.vals.update(kw)\n    self.sudo().write(self.vals)\n";
        assert_eq!(
            scan(Params::Route(&[]), src),
            ["OD704 route parameter 'kw' reaches sudo().write(), so the user picks the fields it sets as superuser"]
        );
    }

    #[test]
    fn a_call_on_self_after_a_store_is_followed() {
        let route = summary(
            Params::Route(&[]),
            "def f(self, term):\n    self.term = term\n    self._h(term)\n",
        );
        let calls: Vec<_> = route.calls.iter().map(|c| c.method.as_str()).collect();
        assert_eq!(calls, ["_h"]);
    }

    #[test]
    fn a_search_or_read_that_reports_stays_a_call_of_the_model() {
        let route = summary(
            Params::Route(&[]),
            "def f(self, t):\n    M = request.env['x.y'].sudo()\n    M.read(t)\n    M.search(t)\n",
        );
        let calls: Vec<_> = route.calls.iter().map(|c| c.method.as_str()).collect();
        assert_eq!(calls, ["read", "search"]);
    }

    /// Whether the first function of `src`, a method of `owner` taking `params`, writes a parameter
    /// on its records.
    fn self_writes(params: Params, owner: Receiver, src: &str) -> bool {
        let parsed = parse_module(src).expect("valid Python");
        let Some(Stmt::FunctionDef(func)) = parsed.syntax().body.first() else {
            panic!("no function");
        };
        let summary = function(
            func,
            params,
            Some(owner),
            None,
            &NAMES,
            &Locator::new(src),
            &TaintTable::default(),
        )
        .1;
        summary.is_some_and(|s| s.edges.iter().any(|e| e.target == FlowTarget::SelfWrite))
    }

    #[test]
    fn a_model_method_writing_a_parameter_on_its_records_is_a_self_write() {
        let model = Receiver::Model("x.y".to_owned());
        for (params, name) in [(Params::None, "_f"), (Params::Rpc, "f")] {
            let write = |owner: &Receiver, call: &str| {
                self_writes(params, owner.clone(), &format!("def {name}(self, vals):\n    {call}\n"))
            };
            assert!(write(&model, "self.write(vals)"), "{name}");
            assert!(write(&model, "self.browse(1).create(vals_list=vals)"), "{name}");
            assert!(write(&model, "self.with_context(a=1).write(vals)"), "{name}");
            assert!(!write(&model, "self.write({'name': vals['name']})"), "{name}");
            assert!(!write(&model, "self.partner_id.write(vals)"), "{name}");
            assert!(!write(&model, "self.env['y'].create(vals)"), "{name}");
            assert!(!write(&model, "self.with_user(1).write(vals)"), "{name}");
            assert!(!write(&model, "self.write(request.params)"), "{name}");
            assert!(!write(&Receiver::Class("C".to_owned()), "self.write(vals)"), "{name}");
        }
    }

    #[test]
    fn a_call_runs_as_its_receiver_shows() {
        let leaves =
            "    L = request.env['x.y'].sudo().with_user(request.env.user)\n    F = request.env['x.y'].sudo(False)\n";
        for (receiver, expected) in [
            ("M", RunsAs::Superuser),
            ("request.env['x.y']", RunsAs::User),
            ("request.env['x.y'].sudo()", RunsAs::Superuser),
            ("request.env['x.y'].sudo(True)", RunsAs::Superuser),
            ("request.env['x.y'].with_user(1).sudo()", RunsAs::Superuser),
            ("request.env['x.y'].sudo().with_user(1)", RunsAs::Superuser),
            ("request.env['x.y'].with_user(SUPERUSER_ID)", RunsAs::Superuser),
            ("request.env(su=True)['x.y']", RunsAs::Superuser),
            ("request.env(user=SUPERUSER_ID)['x.y']", RunsAs::Superuser),
            ("api.Environment(cr, SUPERUSER_ID, {})['x.y']", RunsAs::Superuser),
            ("request.env['x.y'].sudo().with_user(request.env.user)", RunsAs::User),
            (
                "request.env['x.y'].with_user(1).with_user(request.env.user)",
                RunsAs::User,
            ),
            ("request.env['x.y'].sudo(False)", RunsAs::User),
            ("request.env['x.y'].sudo().sudo(False)", RunsAs::User),
            ("request.env['x.y'].sudo().sudo(flag)", RunsAs::User),
            ("L", RunsAs::User),
            ("F", RunsAs::User),
            ("self", RunsAs::AsCaller),
            ("self.with_context(a=1)", RunsAs::AsCaller),
            ("self.with_user(1)", RunsAs::Superuser),
            ("self.with_user(request.env.user)", RunsAs::User),
            ("self.sudo(False)", RunsAs::User),
            ("request.env['x.y'].with_user(1).sudo(False)", RunsAs::Superuser),
            ("request.env['x.y'].sudo(SUPERUSER_ID)", RunsAs::Superuser),
            ("request.env['x.y'].sudo(user=request.env.user)", RunsAs::User),
            ("request.env(su=True)['x.y'].sudo(False)", RunsAs::User),
        ] {
            let route = summary(
                Params::Route(&[]),
                &format!("def f(self, **kw):\n    M = request.env['x.y'].sudo()\n{leaves}    {receiver}._a(kw)\n"),
            );
            let runs_as: Vec<RunsAs> = route.calls.iter().map(|c| c.runs_as).collect();
            assert_eq!(runs_as, [expected], "{receiver}");
        }
    }

    /// The codes `call` reports in a module of `series`, where `t` is a route parameter and `base`
    /// a value the user does not control.
    fn codes_in(series: Option<&str>, call: &str) -> Vec<String> {
        let src = format!("def f(self, t):\n    base = self.get_base_url()\n    {call}\n");
        route_codes_in(series, &src)
    }

    fn codes(call: &str) -> Vec<String> {
        codes_in(None, call)
    }

    #[test]
    fn redirects_requests_and_paths_are_sinks() {
        for (call, code) in [
            ("werkzeug.utils.redirect(t)", "OD705"),
            ("werkzeug.redirect(location=t)", "OD705"),
            ("request.redirect(t, local=False)", "OD705"),
            ("request.redirect(t, 303, False)", "OD705"),
            ("request.redirect_query(t, {}, 303, False)", "OD705"),
            ("request.redirect_query(t, local=False)", "OD705"),
            ("requests.get(t, timeout=5)", "OD706"),
            ("requests.post(url=t)", "OD706"),
            ("requests.Session().put(t)", "OD706"),
            ("requests.request('GET', t)", "OD706"),
            ("requests.request(method='GET', url=t)", "OD706"),
            ("urllib.request.urlopen(t)", "OD706"),
            ("open(t)", "OD707"),
            ("open(file=t)", "OD707"),
            ("io.open(t)", "OD707"),
            ("io.open(file=t)", "OD707"),
            ("http.send_file(filepath_or_fp=t)", "OD707"),
            ("os.path.join(base, t)", "OD707"),
        ] {
            assert_eq!(codes(call), [code], "{call}");
        }
        for call in [
            "request.redirect(t)",
            "request.redirect(t, local=True)",
            "request.redirect_query(t, {})",
            "requests.request(t, base)",
            "Image.open(t)",
            "http.send_file(name=t)",
            "os.path.join(base, 'x')",
            "tools.file_open(t)",
            // A session is followed only when built in the call's own expression.
            "s = requests.Session(); s.get(t)",
            "self.session = requests.Session(); self.session.get(t)",
        ] {
            assert!(codes(call).is_empty(), "{call}");
        }
    }

    #[test]
    fn open_takes_its_path_as_name_on_python_2_only() {
        for series in ["9.0", "10.0"] {
            assert_eq!(codes_in(Some(series), "open(name=t)"), ["OD707"], "{series}");
            assert_eq!(codes_in(Some(series), "open(t)"), ["OD707"], "{series}");
        }
        assert_eq!(codes_in(Some("10.0"), "open(file=t)"), ["OD707"]);
        for series in [Some("11.0"), Some("18.0"), None] {
            assert!(codes_in(series, "open(name=t)").is_empty(), "{series:?}");
            assert_eq!(codes_in(series, "open(file=t)"), ["OD707"], "{series:?}");
        }
    }

    #[test]
    fn a_redirect_is_local_from_15_0_on() {
        for series in ["12.0", "14.0"] {
            assert_eq!(codes_in(Some(series), "request.redirect(t)"), ["OD705"], "{series}");
        }
        for series in [Some("15.0"), Some("18.0"), None] {
            assert!(codes_in(series, "request.redirect(t)").is_empty(), "{series:?}");
        }
        assert_eq!(codes_in(Some("18.0"), "request.redirect(t, local=False)"), ["OD705"]);
    }

    #[test]
    fn a_name_without_its_directories_opens_no_other_file() {
        for call in [
            "open(os.path.basename(t))",
            "io.open(os.path.basename(t))",
            "os.path.join(base, os.path.basename(t))",
            "http.send_file(io.BytesIO(t))",
            "open(werkzeug.utils.secure_filename(t))",
            "open('/tmp/' + basename(t))",
        ] {
            assert!(codes(call).is_empty(), "{call}");
        }
        assert_eq!(codes("open(basename(t) + t)"), ["OD707"]);
        assert_eq!(codes("self.env.cr.execute('SELECT %s' % basename(t))"), ["OD701"]);
        assert!(codes("self.env.cr.execute('SELECT %s' % secure_filename(t))").is_empty());
    }

    #[test]
    fn a_start_the_code_picks_keeps_the_host() {
        for call in [
            "werkzeug.utils.redirect(f'/shop/{t}')",
            "werkzeug.utils.redirect(f'https://example.com/{t}')",
            "werkzeug.utils.redirect(f'{base}{t}')",
            "werkzeug.utils.redirect('/shop?next=' + t)",
            "werkzeug.utils.redirect('#' + t + t)",
            "werkzeug.utils.redirect(base + t)",
            "werkzeug.utils.redirect('/shop/%s' % t)",
            "werkzeug.utils.redirect('%s/%s' % (base, t))",
            "werkzeug.utils.redirect('/shop/{}'.format(t))",
            "werkzeug.utils.redirect('{}/{}'.format(base, t))",
            "werkzeug.utils.redirect('{root}{t}'.format(root=base, t=t))",
            "requests.get('https://api.example.com/v1/%s' % t)",
            "werkzeug.utils.redirect('/shop?' + urllib.parse.urlencode(t))",
            "werkzeug.utils.redirect(werkzeug.urls.url_encode(t))",
        ] {
            assert!(codes(call).is_empty(), "{call}");
        }
        for call in [
            "werkzeug.utils.redirect(f'/{t}')",
            "werkzeug.utils.redirect(f'//{t}')",
            "werkzeug.utils.redirect('/\\\\' + t)",
            "werkzeug.utils.redirect(f'https://{t}')",
            "werkzeug.utils.redirect('https://example.com' + t)",
            "werkzeug.utils.redirect(t + '/shop')",
            "werkzeug.utils.redirect('%s/shop' % t)",
            "werkzeug.utils.redirect('%(a)s/shop' % {'a': t})",
            "werkzeug.utils.redirect('{}/shop'.format(t))",
            "werkzeug.utils.redirect(f'{t}/shop')",
            "werkzeug.utils.redirect(urllib.parse.urljoin(base, t))",
            "open('/tmp/' + t)",
        ] {
            assert_eq!(codes(call).len(), 1, "{call}");
        }
    }

    #[test]
    fn calls_that_keep_some_arguments() {
        for call in [
            "request.redirect(getattr(request.website, 'social_%s' % t, False), local=False)",
            "request.redirect(_build_url_w_params(base, {'next': t}), local=False)",
        ] {
            assert!(codes(call).is_empty(), "{call}");
        }
        for call in [
            "request.redirect(getattr(t, 'url'), local=False)",
            "request.redirect(getattr(base, 'url', t), local=False)",
            "request.redirect(_build_url_w_params(t, {}), local=False)",
            "request.redirect(_build_url_w_params(url_string=t, query_params={}), local=False)",
            "open(self.base_re.match(t).group('rest'))",
            "pat = self.base_re\n    open(pat.fullmatch(t)[1])",
        ] {
            assert_eq!(codes(call).len(), 1, "{call}");
        }
    }

    #[test]
    fn a_checked_start_keeps_the_host() {
        let redirect = |check: &str| {
            scan(
                Params::Route(&[]),
                &format!("def f(self, t):\n{check}\n    return werkzeug.utils.redirect(t)\n"),
            )
        };
        assert!(redirect("    if not t.startswith('/shop'):\n        raise ValueError()").is_empty());
        assert!(redirect("    assert t.startswith(('/web', 'https://example.com/'))").is_empty());
        assert_eq!(
            redirect("    if not t.startswith('/'):\n        raise ValueError()").len(),
            1
        );
        assert_eq!(
            redirect("    if t.startswith('/shop'):\n        raise ValueError()").len(),
            1
        );
        assert_eq!(redirect("    assert t.startswith(('/web', '//'))").len(), 1);
    }

    #[test]
    fn a_value_harmless_on_one_branch_only_reaches_the_sink() {
        let route = summary(
            Params::None,
            "def _h(self, t):\n    if self:\n        return '/shop/' + t\n    return t\n",
        );
        assert_eq!(route.edges.len(), 1);
        assert!(!route.edges[0].safe.has("OD705"));
    }

    #[test]
    fn commands_loads_parsers_and_headers_are_sinks() {
        for (call, code) in [
            ("os.system(t)", "OD708"),
            ("os.popen(cmd=t)", "OD708"),
            ("subprocess.getoutput(t)", "OD708"),
            ("subprocess.getstatusoutput(t)", "OD708"),
            ("subprocess.run(t, shell=True)", "OD708"),
            ("subprocess.call(['sh', t], shell=True)", "OD708"),
            ("subprocess.check_call(args=t, shell=True)", "OD708"),
            ("subprocess.check_output(t, shell=True)", "OD708"),
            (
                "subprocess.Popen(t, -1, None, None, None, None, None, True, True)",
                "OD708",
            ),
            ("pickle.loads(t)", "OD709"),
            ("pickle.load(file=t)", "OD709"),
            ("marshal.loads(t)", "OD709"),
            ("marshal.load(t)", "OD709"),
            ("yaml.unsafe_load(t)", "OD709"),
            ("yaml.unsafe_load_all(t)", "OD709"),
            (
                "lxml.etree.fromstring(t, etree.XMLParser(resolve_entities=True))",
                "OD710",
            ),
            ("etree.XML(t, parser=etree.XMLParser(resolve_entities=True))", "OD710"),
            ("etree.parse(t, etree.XMLParser(resolve_entities=True))", "OD710"),
            (
                "objectify.fromstring(t, etree.XMLParser(resolve_entities=True))",
                "OD710",
            ),
            ("request.make_response(base, [('X-Name', t)])", "OD711"),
            ("request.make_json_response(base, headers={'X-Name': t})", "OD711"),
            ("Response(base, 200, [('X-Name', t)])", "OD711"),
            ("Response(base, headers=[('X-Name', t)])", "OD711"),
            ("base.headers.add('X-Name', t)", "OD711"),
            ("base.headers.set('X-Name', t)", "OD711"),
            ("base.headers['X-Name'] = t", "OD711"),
            ("resp = request.make_response(base)\n    resp.headers['X'] = t", "OD711"),
            (
                "res = request.make_response(base)\n    res.headers.set('X', t)",
                "OD711",
            ),
            ("client.headers['X'] = t", "OD711"),
            (
                "s = requests.Session()\n    s = request.make_response(base)\n    s.headers['X'] = t",
                "OD711",
            ),
            (
                "self.client = httpx.Client()\n    self.client = request.make_response(base)\n    self.client.headers['X'] = t",
                "OD711",
            ),
            (
                "client = request.make_response(base)\n    client.headers['X'] = t",
                "OD711",
            ),
            (
                "r = request.make_response(base)\n    if base:\n        r = requests.Session()\n    r.headers['X'] = t",
                "OD711",
            ),
            (
                "if base:\n        r = requests.Session()\n    else:\n        r = request.make_response(base)\n    r.headers['X'] = t",
                "OD711",
            ),
            (
                "try:\n        r = requests.Session()\n    except Exception:\n        r = request.make_response(base)\n    r.headers['X'] = t",
                "OD711",
            ),
            (
                "r = request.make_response(base)\n    for x in base:\n        r = requests.Session()\n    r.headers['X'] = t",
                "OD711",
            ),
        ] {
            assert_eq!(codes(call), [code], "{call}");
        }
        for call in [
            "subprocess.run(t)",
            "subprocess.run(t, shell=False)",
            "subprocess.Popen(t, -1, None, None, None, None, None, True, False)",
            "subprocess.run('ls', shell=True, input=t)",
            "request.make_response(t)",
            "request.make_response(base, [('X-Name', base)], t)",
            "Response(t, 200)",
            "headers = {}\n    headers['X-Name'] = t",
            "self.session.headers['Authorization'] = t",
            "requests.Session().headers['Authorization'] = t",
            "session.headers.update({'Authorization': t})",
            "self._session.headers.set('Authorization', t)",
            "s = requests.Session()\n    s.headers['Authorization'] = t",
            "sess = requests.session()\n    sess.headers.set('Authorization', t)",
            "self.client = httpx.Client()\n    self.client.headers['Authorization'] = t",
            "c = httpx.AsyncClient()\n    c.headers.add('X', t)",
            "httpx.Client().headers['X'] = t",
            "with requests.Session() as s:\n        s.headers['X'] = t",
            "if base:\n        r = requests.Session()\n    else:\n        r = httpx.Client()\n    r.headers['X'] = t",
        ] {
            assert!(codes(call).is_empty(), "{call}");
        }
    }

    #[test]
    fn a_superuser_environment_leaves_the_other_sinks_as_they_were() {
        let locals = "env = request.env(su=True)\n    R = request.env['x'].sudo().with_user(request.env.user)\n    ";
        for (call, code) in [
            ("env.cr.execute(t)", "OD701"),
            ("R.env.cr.execute(t)", "OD701"),
            ("request.env(su=True).cr.execute(t)", "OD701"),
            ("api.Environment(env.cr, SUPERUSER_ID, {}).cr.execute(t)", "OD701"),
            ("safe_eval(t, {'env': env, 'records': R})", "OD702"),
            ("Markup(t)", "OD703"),
            ("request.redirect(t, local=False)", "OD705"),
            ("requests.get(t, timeout=5)", "OD706"),
            ("open(t)", "OD707"),
            ("os.system(t)", "OD708"),
            ("pickle.loads(t)", "OD709"),
            ("etree.fromstring(t, etree.XMLParser(resolve_entities=True))", "OD710"),
            ("request.make_response(base, [('X-Name', t)])", "OD711"),
        ] {
            assert_eq!(codes(&format!("{locals}{call}")), [code], "{call}");
        }
    }

    #[test]
    fn a_safe_yaml_loader_builds_plain_values() {
        for loader in ["yaml.Loader", "yaml.UnsafeLoader", "yaml.FullLoader", "loader"] {
            assert_eq!(codes(&format!("yaml.load(t, Loader={loader})")), ["OD709"], "{loader}");
        }
        assert_eq!(codes("yaml.load(t)"), ["OD709"]);
        assert_eq!(
            codes("Loader = yaml.Loader\n    yaml.load(t, Loader=Loader)"),
            ["OD709"]
        );
        let either = "Loader = yaml.SafeLoader\n    if base:\n        Loader = yaml.Loader\n    yaml.load(t, Loader)";
        assert_eq!(codes(either), ["OD709"]);
        for bound in [
            "Loader = getattr(yaml, 'CSafeLoader', yaml.SafeLoader)\n    yaml.load(t, Loader=Loader)",
            "Loader = yaml.SafeLoader\n    L = Loader\n    yaml.load(t, L)",
            "yaml.load(t, Loader=getattr(yaml, 'CSafeLoader', yaml.SafeLoader))",
        ] {
            assert!(codes(bound).is_empty(), "{bound}");
        }
        assert_eq!(codes("yaml.load_all(t, yaml.Loader)"), ["OD709"]);
        for loader in ["SafeLoader", "CSafeLoader", "BaseLoader", "CBaseLoader"] {
            assert!(
                codes(&format!("yaml.load(t, Loader=yaml.{loader})")).is_empty(),
                "{loader}"
            );
            assert!(codes(&format!("yaml.load_all(t, {loader})")).is_empty(), "{loader}");
        }
    }

    #[test]
    fn the_default_parser_resolves_entities_before_saas_19_1() {
        let parse = |series: Option<&str>, call: &str| codes_in(series, call).len();
        for call in [
            "etree.fromstring(t)",
            "etree.fromstring(t, None)",
            "etree.fromstring(t, parser=etree.XMLParser(remove_blank_text=True))",
            "p = etree.XMLParser()\n    etree.fromstring(t, p)",
        ] {
            assert_eq!(parse(Some("18.0"), call), 1, "{call}");
            assert_eq!(parse(Some("saas~19.1"), call), 0, "{call}");
            assert_eq!(parse(None, call), 0, "{call}");
        }
        for call in [
            "etree.fromstring(t, etree.XMLParser(resolve_entities=True))",
            "p = etree.XMLParser(resolve_entities=True)\n    etree.fromstring(t, parser=p)",
        ] {
            for series in [Some("18.0"), Some("saas~19.1"), None] {
                assert_eq!(parse(series, call), 1, "{call} on {series:?}");
            }
        }
        for call in [
            "etree.fromstring(t, etree.XMLParser(resolve_entities=False))",
            "p = etree.XMLParser(resolve_entities=False)\n    etree.fromstring(t, p)",
            "etree.fromstring(t, etree.HTMLParser())",
            "etree.fromstring(t, self.parser)",
            "etree.parse(tools.file_open(t))",
        ] {
            assert_eq!(parse(Some("18.0"), call), 0, "{call}");
        }
        let either = "p = etree.XMLParser(resolve_entities=True)\n    if base:\n        p = etree.XMLParser(resolve_entities=False)\n    etree.fromstring(t, p)";
        assert_eq!(parse(None, either), 1);
        // A sink too, the tree keeps the taint of the text.
        assert_eq!(
            codes_in(
                Some("18.0"),
                "root = etree.fromstring(t)\n    self.env.cr.execute('SELECT %s' % root.findtext('q'))\n    Markup(etree.tostring(root))"
            ),
            ["OD710", "OD701", "OD703"]
        );
        assert_eq!(
            codes("data = pickle.loads(t)\n    safe_eval(data['x'])"),
            ["OD709", "OD702"]
        );
        assert_eq!(
            codes("out = subprocess.check_output(t, shell=True)\n    self.env.cr.execute('SELECT %s' % out)"),
            ["OD708", "OD701"]
        );
        // `result` keeps the arguments of a call on a name the function never binds.
        assert_eq!(
            codes("r = request.make_response(t, [('X-Name', t)])\n    self.env.cr.execute('SELECT %s' % r)"),
            ["OD711", "OD701"]
        );
        // No sink, so the tree keeps the taint of the text, as a module's function's result does.
        assert_eq!(
            codes_in(
                Some("18.0"),
                "Markup(etree.tostring(etree.fromstring(t, etree.HTMLParser())))"
            ),
            ["OD703"]
        );
    }

    #[test]
    fn the_pickle_of_odoo_loads_plain_data() {
        let load = |imports: &str, call: &str| {
            let src = format!("{imports}\ndef f(self, t):\n    {call}\n");
            scan(Params::Route(&[]), &src).len()
        };
        assert_eq!(load("import pickle", "pickle.loads(t)"), 1);
        assert_eq!(load("from odoo.tools import pickle", "pickle.loads(t)"), 0);
        assert_eq!(load("from odoo.tools.misc import pickle", "pickle.load(t)"), 0);
        assert_eq!(load("from odoo import tools", "tools.pickle.loads(t)"), 0);
        assert_eq!(load("from odoo.tools import pickle", "marshal.loads(t)"), 1);
    }

    #[test]
    fn quoting_and_file_names_end_these_flows() {
        for call in [
            "os.system('lpr ' + shlex.quote(t))",
            "subprocess.run(shlex.join(['lpr', t]), shell=True)",
            "request.make_response(base, [('Content-Disposition', content_disposition(t))])",
            "base.headers['Content-Disposition'] = http.content_disposition(t)",
        ] {
            assert!(codes(call).is_empty(), "{call}");
        }
        assert_eq!(codes("self.env.cr.execute('SELECT ' + shlex.quote(t))"), ["OD701"]);
    }

    #[test]
    fn only_a_file_name_header_asks_for_content_disposition() {
        let message = |call: &str| {
            let src = format!("def f(self, t):\n    {call}\n");
            let parsed = parse_module(&src).expect("valid Python");
            let Some(Stmt::FunctionDef(func)) = parsed.syntax().body.first() else {
                panic!("no function");
            };
            let loc = Locator::new(&src);
            let (diags, _) = function(
                func,
                Params::Route(&[]),
                None,
                None,
                &NAMES,
                &loc,
                &TaintTable::default(),
            );
            diags.into_iter().map(|d| d.message).collect::<Vec<_>>().join("\n")
        };
        assert_eq!(
            message("r.headers['Content-Type'] = t"),
            "route parameter 't' reaches the Content-Type header of r.headers, so the user picks how the browser reads the response; pick the value from those the code allows"
        );
        assert!(message("r.headers.add(base, t)").contains("reaches a header of headers.add(), so"));
        for call in [
            "r.headers['content-disposition'] = t",
            "r.headers.set('Content-Disposition', t)",
            "request.make_response('', [('Content-Type', 'text/csv'), ('Content-Disposition', 'a; filename=' + t)])",
            "request.make_response('', headers={'Content-Disposition': f'a; filename={t}'})",
            "h = [('Content-Disposition', 'a; filename=' + t)]\n    request.make_response(base, h)",
            "h = {'Content-Disposition': f'a; filename={t}'}\n    request.make_response(base, headers=h)",
            "request.make_response(base, [('Content-Length', len(base)), ('Content-Disposition', 'a; filename=' + t)])",
            // The shape of 18.0 addons/mail/controllers/attachment.py:40-46: two entries are not
            // literals, and only one is tainted.
            "body = base.encode()\n    headers = [\n        ('Cache-Control', 'no-store'),\n        ('X-Body-Size', str(body.count(b'x'))),\n        ('Content-Disposition', 'attachment; filename=' + t),\n    ]\n    request.make_response(body, headers)",
            "request.make_response(t, [('Content-Length', len(t)), ('Content-Disposition', 'a; filename=' + request.params['n'])])",
            "self.n = t\n    request.make_response(base, [('Content-Length', len(t)), ('Content-Disposition', 'a; filename=' + self.n)])",
            "key = 'Content-Disposition'\n    r.headers.set(key, t)",
            "key = 'Content-Disposition'\n    r.headers[key] = t",
        ] {
            assert!(
                message(call).ends_with("; pass the file name through content_disposition()"),
                "{call}"
            );
        }
        for call in [
            "request.make_response('', [('Content-Type', t), ('Content-Disposition', 'a; filename=' + t)])",
            "h = {'Content-Disposition': 'a; filename=' + t}\n    h['Content-Type'] = t\n    request.make_response(base, headers=h)",
            "h = [('Content-Disposition', 'a; filename=' + t)]\n    if base:\n        h = [('X-Name', t)]\n    request.make_response(base, h)",
            "request.make_response(base, headers={**request.params, 'X-Lines': str(len(base))})",
            "request.make_response(base, [*t, ('X-Lines', str(len(base)))])",
        ] {
            let found = message(call);
            assert!(found.contains("reaches a header of make_response()"), "{call}: {found}");
        }
    }

    #[test]
    fn a_route_parameter_a_converter_fills_with_records_is_no_source() {
        let src = "def f(self, partner, page):\n    os.system(partner.name)\n    os.system(page)\n";
        assert_eq!(
            scan(Params::Route(&["partner"]), src),
            ["OD708 route parameter 'page' reaches the command of os.system(), so the user picks what the shell runs"]
        );
    }

    /// Each rule's sink, and the route value it reports: `**kw` for OD704, which needs a mapping.
    fn loop_sinks() -> impl Iterator<Item = (&'static str, &'static str, &'static str)> {
        let sudo_write = ("OD704", "self.env['res.partner'].sudo().write(#)", "kw");
        SINK_OF
            .iter()
            .map(|&(code, sink)| (code, sink, "t"))
            .chain([sudo_write])
    }

    /// The codes of a route of parameters `t` and `**kw` running `body`, where `<loop>` opens a
    /// loop over records as `x`, `<sink>` is `sink` on `arg` and `<t>` the route value.
    fn looped(body: &str, sink: &str, arg: &str, t: &str) -> Vec<String> {
        let body = body
            .replace("<loop>", "for x in self.env['x.y'].search([]):")
            .replace("<sink>", &sink.replace('#', arg))
            .replace("<t>", t);
        route_codes(&format!("def f(self, t, **kw):\n    {body}\n"))
    }

    #[test]
    fn loops_carry_taint_around() {
        for (code, sink, t) in loop_sinks() {
            // A return in the body would leave the loop before a second pass, so OD716 keeps
            // the records it browses there and returns them after the loop.
            let (sink, after) = match sink.strip_prefix("return ") {
                Some(call) => (format!("r = {call}"), "\n    return r"),
                None => (sink.to_owned(), ""),
            };
            let late = format!("v = ''\n    <loop>\n        <sink>\n        v = <t>{after}");
            let while_loop = format!("v = ''\n    while self:\n        <sink>\n        v = <t>{after}");
            for body in [late, while_loop] {
                assert_eq!(looped(&body, &sink, "v", t), [code], "{code}: {body}");
            }
        }
        let vals = "vals = {'public': True}\n    <loop>\n        <sink>\n        vals['raw'] = <t>";
        let create = "self.env['ir.attachment'].sudo().create(#)";
        assert_eq!(looped(vals, create, "vals", "t"), ["OD717"]);
    }

    /// OD704 is left out: an element or an attribute holds a value of the mapping, not its keys.
    #[test]
    fn a_route_value_in_a_loop_is_reported_for_each_rule_but_od704() {
        // The last case reads after the loop what only the first pass set on its target.
        let cases = [
            ("for x in <t>:\n        <sink>", "x"),
            ("for k, v in kw.items():\n        <sink>", "v"),
            ("<loop>\n        x.b = <t>\n        <sink>", "x.b"),
            ("<loop>\n        x.b = <t>\n    <sink>", "x.b"),
            (
                "v = <t>\n    <loop>\n        x.b = v\n        v = ''\n    <sink>",
                "x.a",
            ),
            ("v = <t>\n    <loop>\n        y = v\n        v = ''\n    <sink>", "y"),
            (
                "v = <t>\n    while self:\n        y = v\n        v = ''\n    <sink>",
                "y",
            ),
        ];
        for (code, sink) in SINK_OF {
            for (body, arg) in cases {
                assert_eq!(looped(body, sink, arg, "t"), [*code], "{code}: {body}");
            }
        }
    }

    /// What a loop body reads from its target `x` and sets on it for `code`: OD704 needs the keys
    /// of a mapping, which only an update adds.
    fn read_and_set(code: &str) -> (&'static str, &'static str) {
        if code == "OD704" {
            ("x", "x.update(<t>)")
        } else {
            ("x.a", "x.b = <t>")
        }
    }

    #[test]
    fn a_for_target_is_bound_again_before_each_pass() {
        for (code, sink, t) in loop_sinks() {
            let (read, set) = read_and_set(code);
            let mut sets = vec![set];
            if matches!(code, "OD701" | "OD707") {
                sets.extend(["x['b'] = <t>", "x = <t>"]);
            }
            for set in sets {
                let body = format!("<loop>\n        <sink>\n        {set}");
                assert!(looped(&body, sink, read, t).is_empty(), "{code}: {body}");
            }
            // The shape of `_get_icon_image` in `ir_module.py`, which reads an attribute of a
            // module and then sets another from a call.
            let attr_set = set.replace("<t>", "self._x()").replace("x.b", "x.icon_flag");
            let body = format!(
                "for x in self:\n        {}\n        {attr_set}",
                sink.replace('#', &read.replace(".a", ".icon"))
            );
            let (_, edges) = flows_of(Params::None, &format!("def _f(self):\n    {body}\n"));
            // Outside a route OD716 keeps no edge to its sink, so the call it browses with stands
            // in for it.
            let reached = |t: &FlowTarget| match code {
                "OD716" => matches!(t, FlowTarget::Arg(..)),
                _ => *t == FlowTarget::Sink(code),
            };
            assert!(origins_where(&edges, reached).is_empty(), "{code}: {body}");
        }
    }

    /// The codes of a function body where `t` is a route parameter and `P` the records of a model
    /// the code picks.
    fn codes_of(body: &str) -> Vec<String> {
        codes_after("", body)
    }

    /// The codes of `codes_of(body)` in a module whose `imports` come before the function.
    fn codes_after(imports: &str, body: &str) -> Vec<String> {
        route_codes(&format!(
            "{imports}\ndef f(self, t):\n    P = self.env['res.partner']\n{body}\n"
        ))
    }

    #[test]
    fn models_methods_fields_and_domains_are_sinks() {
        for (call, code) in [
            ("self.env[t].sudo().search([])", "OD712"),
            ("request.env.get(t).sudo().browse(1).unlink()", "OD712"),
            ("self.env[t].with_context(a=1).sudo().exists().write({'a': 1})", "OD712"),
            ("self.env[t].sudo().filtered('active').mapped('name')", "OD712"),
            ("self.env[t].sudo()._read_group([], ['a'])", "OD712"),
            ("getattr(P, t)()", "OD713"),
            ("getattr(P.sudo(), '_' + t)(1)", "OD713"),
            ("P.sudo()[t]", "OD714"),
            ("P.sudo().browse(1).read([t])", "OD714"),
            ("P.sudo().read(fields=t)", "OD714"),
            ("P.sudo().mapped(t)", "OD714"),
            ("P.sudo().search(t)", "OD715"),
            ("P.sudo().search_count(domain=t)", "OD715"),
            ("P.sudo().search_read([], t)", "OD715"),
            ("P.sudo().search_fetch([], field_names=[t])", "OD715"),
            ("P.sudo().search([(t, '=', 1)])", "OD715"),
            ("P.sudo().search([('a', t, 1)])", "OD715"),
            ("P.sudo().search([('child_ids', 'any', t)])", "OD715"),
            ("P.sudo().search([('child_ids', 'not any', t)])", "OD715"),
            ("P.sudo().search(Domain('child_ids', 'any!', t))", "OD715"),
            ("P.sudo().search(Domain(t, '=', 1))", "OD715"),
            ("P.sudo().search([('a', '=', 1), [t, '=', 1]])", "OD715"),
            ("request.env(su=True)[t].search([])", "OD712"),
            ("self.env[t].with_user(SUPERUSER_ID).search([])", "OD712"),
            ("env = request.env(su=True)\n    env[t].search([])", "OD712"),
            ("getattr(P.sudo().with_user(request.env.user), t)()", "OD713"),
            ("request.env(su=True)['x'][t]", "OD714"),
            ("M = request.env(su=True)['x']\n    M[t]", "OD714"),
            ("P.with_user(1)[t]", "OD714"),
            ("request.env(su=True)['x'].search(t)", "OD715"),
            ("M = request.env(su=True)['x']\n    M.search(t)", "OD715"),
            ("P.with_user(SUPERUSER_ID).search(t)", "OD715"),
        ] {
            assert_eq!(codes_of(&format!("    {call}")), [code], "{call}");
        }
        for call in [
            "self.env[t].search([])",
            "self.env[t].sudo()._get_mail_message_access([1], 'read')",
            "self.env['x'].sudo().search([])",
            "getattr(P, t)",
            "getattr(P, 'name')()",
            "P[t]",
            "P.read([t])",
            "P.search(t)",
            "P.sudo().search([('name', 'ilike', t)])",
            "P.sudo().search(['|', ('a', '=', t), ['b', 'in', t]])",
            "P.sudo().search(Domain('name', '=', t) | Domain('ref', '=', t))",
            "P.sudo().search(expression.AND([[('a', '=', 1)], [('name', '=', t)]]))",
            "P.sudo().search([], order=t)",
            "request.env(su=False)[t].search([])",
            "self.env[t].sudo().with_user(request.env.user).search([])",
            "self.env[t].sudo(flag).search([])",
            "M = request.env[t].sudo().with_user(request.env.user)\n    M.search([])",
            "P.sudo().with_user(request.env.user).search(t)",
            "M = request.env(su=True)['x'].with_user(request.env.user)\n    M.search(t)",
        ] {
            assert!(codes_of(&format!("    {call}")).is_empty(), "{call}");
        }
    }

    #[test]
    fn a_domain_joined_with_and_keeps_the_user_side() {
        for domain in [
            "base & Domain(json.loads(t))",
            "Domain('active', '=', True) & Domain(t)",
            "Domain(t) & self._base()",
            "base & ~Domain(t)",
            "~Domain(t)",
            "Domain.TRUE & Domain(t)",
            "(base & Domain('a', '=', 1)) & Domain(t)",
            "base | Domain(t)",
        ] {
            assert_eq!(
                codes_of(&format!("    P.sudo().search({domain})")),
                ["OD715"],
                "{domain}"
            );
        }
        for domain in [
            "base & Domain('name', 'ilike', t)",
            "~Domain('name', '=', t)",
            "Domain('active', '=', True) & Domain('name', '=', t)",
            "t & base",
        ] {
            assert!(
                codes_of(&format!("    P.sudo().search({domain})")).is_empty(),
                "{domain}"
            );
        }
    }

    #[test]
    fn both_read_group_signatures_check_the_fields() {
        for call in [
            "P.sudo().read_group([], [t], ['a'])",
            "P.sudo().read_group([], ['a'], [t])",
            "P.sudo().read_group([], ['a'], ['b'], t)",
            "P.sudo().read_group([], groupby=[t])",
            "P.sudo().read_group([], fields=[t], groupby=['a'])",
            "P.sudo()._read_group([], aggregates=[t])",
            "P.sudo().formatted_read_group([(t, '=', 1)], ['a'])",
        ] {
            let expected: &[&str] = if call.ends_with(", t)") { &[] } else { &["OD715"] };
            assert_eq!(codes_of(&format!("    {call}")), expected, "{call}");
        }
    }

    #[test]
    fn a_name_built_with_text_of_the_code_picks_the_method_family() {
        for name in [
            "'_get_%s_view' % t",
            "t + '_rate'",
            "f'a_{t}'",
            "'x_{}'.format(t)",
            "'_' + t + '_rate'",
        ] {
            assert!(codes_of(&format!("    getattr(P, {name})()")).is_empty(), "{name}");
        }
        for name in [
            "'_' + t",
            "'%s' % t",
            "f'_{t}'",
            "'{}'.format(t)",
            "'%(a)s' % {'a': t}",
            "'_%%_%s' % t",
        ] {
            assert_eq!(codes_of(&format!("    getattr(P, {name})()")), ["OD713"], "{name}");
        }
        assert_eq!(
            codes_of("    self.env.cr.execute('SELECT %s' % (t + '_rate'))"),
            ["OD701"]
        );
    }

    #[test]
    fn locals_keep_the_model_the_user_picks() {
        let picked = "    M = request.env[t]\n    R = M.sudo()\n    S = R.browse(1)\n";
        assert_eq!(codes_of(&format!("{picked}    S.read(['name'])")), ["OD712"]);
        assert_eq!(codes_of(&format!("{picked}    R.create({{}})")), ["OD712"]);
        assert!(codes_of(&format!("{picked}    M.search([])")).is_empty());
        assert!(codes_of(&format!("{picked}    S.check_access('read')")).is_empty());
        let rebound = "    M = request.env[t]\n    M = self.env['res.partner']\n    M.sudo().search([])";
        assert!(codes_of(rebound).is_empty());
        let branch = "    M = self.env['res.partner']\n    if t:\n        M = request.env[t]\n    M.sudo().search([])";
        assert_eq!(codes_of(branch), ["OD712"]);
    }

    #[test]
    fn an_existing_model_or_field_is_still_one_the_user_picks() {
        let raises = "\n        raise ValueError()";
        for (check, call, code) in [
            ("t not in request.env", "self.env[t].sudo().search([])", "OD712"),
            ("t not in P._fields", "P.sudo()[t]", "OD714"),
            ("t not in self.pool", "P.sudo().search([(t, '=', 1)])", "OD715"),
        ] {
            let body = format!("    if {check}:{raises}\n    {call}");
            assert_eq!(codes_of(&body), [code], "{check}");
        }
        let checked = format!("    if t not in P._fields:{raises}\n    self.env.cr.execute('SELECT %s' % t)");
        assert!(codes_of(&checked).is_empty());
        let allowed = format!("    if t not in ('name', 'ref'):{raises}\n    P.sudo()[t]");
        assert!(codes_of(&allowed).is_empty());
        let negated = format!("    if not t in ('name', 'ref'):{raises}\n    P.sudo()[t]");
        assert!(codes_of(&negated).is_empty());
        let asserted = "    assert not (t in ('name', 'ref'))\n    P.sudo()[t]";
        assert_eq!(codes_of(asserted), ["OD714"]);
    }

    /// `codes_of` in a file that binds `ALLOWED` to a tuple of field names.
    fn allowed_codes(body: &str) -> Vec<String> {
        codes_after("ALLOWED = ('name', 'ref')\n", body)
    }

    #[test]
    fn an_arm_confines_a_value_to_a_container_the_code_chose() {
        let sink = "P.sudo()[t]";
        for (before, test) in [
            ("", "t in ('name', 'ref')"),
            ("", "t in {'name': 1, 'ref': 2}"),
            ("    allowed = ['name', 'ref']\n", "t in allowed"),
            ("    allowed = self._inputs()\n", "t in allowed"),
            ("", "t in ALLOWED"),
            ("", "t in self._ALLOWED"),
            ("", "t in cls._ALLOWED"),
            ("", "t in self._inputs()"),
            ("", "t in self._inputs('portal')"),
            ("", "flag and t in ALLOWED"),
            (
                "    allowed = [f for f in ALLOWED if f != 'password']\n",
                "t in allowed",
            ),
            ("", "t in [f for f in ALLOWED]"),
            ("    name = 'ref'\n    allowed = [name, 'x']\n", "t in allowed"),
            ("    allowed = self._pick(t)\n    allowed = ALLOWED\n", "t in allowed"),
        ] {
            let body = format!("{before}    if {test}:\n        {sink}");
            assert!(allowed_codes(&body).is_empty(), "{body}");
        }
        for body in [
            format!("    if not t:\n        return\n    elif t in ('name', 'ref'):\n        {sink}"),
            format!("    if t not in ('name', 'ref'):\n        return\n    else:\n        {sink}"),
            format!("    if t not in ('name', 'ref'):\n        pass\n    elif flag:\n        {sink}"),
            format!("    if t not in ALLOWED:\n        t = 'name'\n    {sink}"),
            "    if t in ('name', 'ref'):\n        self.env.cr.execute('SELECT %s' % t)".to_owned(),
        ] {
            assert!(allowed_codes(&body).is_empty(), "{body}");
        }
    }

    #[test]
    fn a_value_stays_tainted_where_its_membership_may_not_hold() {
        let sink = "P.sudo()[t]";
        for body in [
            format!("    if t in ('name', 'ref'):\n        pass\n    {sink}"),
            format!("    if t in ('name', 'ref'):\n        pass\n    else:\n        {sink}"),
            format!("    if t in ('name', 'ref'):\n        pass\n    elif flag:\n        {sink}"),
            format!("    if t not in ('name', 'ref'):\n        {sink}"),
            format!("    if not t in ('name', 'ref'):\n        {sink}"),
            format!("    if t in ('name', 'ref') or flag:\n        {sink}"),
            format!("    if (t in ALLOWED) is False:\n        {sink}"),
            format!("    if t in request.params:\n        {sink}"),
            format!("    if t in self._pick(t):\n        {sink}"),
            format!("    if t in self.mapped('name'):\n        {sink}"),
            format!("    if t in self._name:\n        {sink}"),
            format!("    if t in self._context:\n        {sink}"),
            format!("    partner = P.browse(1)\n    if t in partner.name:\n        {sink}"),
            format!("    if t in ALLOWED[:1]:\n        {sink}"),
            format!("    if t in 'name,ref'.lower():\n        {sink}"),
            format!("    allowed = request.params.get('a')\n    if t in allowed:\n        {sink}"),
            format!("    allowed = self._pick(t)\n    if t in allowed:\n        {sink}"),
            format!("    allowed = [f for f in self._pick(t)]\n    if t in allowed:\n        {sink}"),
            format!(
                "    allowed = ALLOWED\n    if flag:\n        allowed = self._pick(t)\n    if t in allowed:\n        {sink}"
            ),
            format!("    allowed = list(ALLOWED)\n    if t in allowed:\n        {sink}"),
            format!(
                "    allowed = ['name']\n    for f in t:\n        allowed.append(f)\n    if t in allowed:\n        {sink}"
            ),
            format!("    allowed = {{'name': 1}}\n    allowed[t] = 1\n    if t in allowed:\n        {sink}"),
            "    x = request.params['x']\n    if t not in request.params:\n        raise ValueError()\n    if x in t:\n        P.sudo()[x]".to_owned(),
            format!("    allowed = 'name,ref'\n    if t in allowed:\n        {sink}"),
            format!("    if t in ','.join(ALLOWED):\n        {sink}"),
            format!("    if t in 'name,ref':\n        {sink}"),
            format!("    if t in ('name'):\n        {sink}"),
            format!("    if t in str(ALLOWED):\n        {sink}"),
            format!("    if t in f'{{t}},ref':\n        {sink}"),
            format!("    if t in P._fields:\n        {sink}"),
            format!("    if t in P._fields.keys():\n        {sink}"),
            format!("    if t in list(P._fields):\n        {sink}"),
            format!("    if t in P.fields_get():\n        {sink}"),
            format!("    if t in self.fields_get().keys():\n        {sink}"),
            format!("    model_fields = P._fields\n    if t in model_fields:\n        {sink}"),
            format!("    all_fields = P.fields_get()\n    if t in all_fields:\n        {sink}"),
        ] {
            assert_eq!(allowed_codes(&body), ["OD714"], "{body}");
        }
        for added in [
            "allowed.append('ref')",
            "allowed.extend(request.params.get('a'))",
            "allowed.insert(0, request.params['a'])",
            "allowed.add(t)",
            "allowed['ref'] = 1",
        ] {
            let body = format!("    allowed = ['name']\n    {added}\n    if t in allowed:\n        {sink}");
            assert_eq!(allowed_codes(&body), ["OD714"], "{body}");
        }
        let search = "P.sudo().search([(t, '=', 1)])";
        let aliased = format!("    model_fields = P._fields\n    if t in model_fields:\n        {search}");
        assert_eq!(allowed_codes(&aliased), ["OD715"]);
        assert_eq!(
            codes_of("    if t in self.env:\n        self.env[t].sudo().search([])"),
            ["OD712"]
        );
        let text =
            "ALLOWED = 'name,ref'\n\n\ndef f(self, t):\n    if t in ALLOWED:\n        self.env['x.y'].sudo()[t]\n";
        assert_eq!(route_codes(text), ["OD714"]);
        let after = "    if t in ('all', 'name'):\n        P.sudo().search([(t, '=', 1)])\n    P.sudo().search_count([(t, '=', 1)])";
        let src = format!("def f(self, t):\n    P = self.env['x.y']\n{after}\n");
        let found = scan(Params::Route(&[]), &src);
        assert!(
            matches!(&found[..], [one] if one.starts_with("OD715") && one.contains("search_count")),
            "{found:?}"
        );
    }

    #[test]
    fn a_comprehension_builds_its_element_where_its_tests_hold() {
        let allowed = "    P.sudo().read([f for f in t if f in ('name', 'ref')])";
        assert!(codes_of(allowed).is_empty());
        let existing = "    P.sudo().read([f for f in t if f in P._fields])";
        assert_eq!(codes_of(existing), ["OD714"]);
        let filtered = "    P.sudo().read([f for f in t if f])";
        assert_eq!(codes_of(filtered), ["OD714"]);
    }

    #[test]
    fn a_guard_confines_a_value_to_any_container() {
        let sink = "P.sudo()[t]";
        for body in [
            format!("    assert t in request.params\n    {sink}"),
            format!("    if t not in request.params:\n        raise ValueError()\n    {sink}"),
            format!("    if t not in self._pick(t):\n        raise ValueError()\n    {sink}"),
            format!("    if t not in P._fields.keys():\n        raise ValueError()\n    {sink}"),
            "    P.sudo().read([f for f in t if f in request.params])".to_owned(),
        ] {
            assert!(codes_of(&body).is_empty(), "{body}");
        }
    }

    #[test]
    fn a_condition_blames_its_field_and_not_its_value() {
        let sinks = |src: &str| -> Vec<FlowOrigin> {
            let found = summary(Params::None, src);
            let sinks = found.edges.iter().filter(|e| e.target == FlowTarget::Sink("OD715"));
            sinks.map(|e| e.origin).collect()
        };
        let condition = "def _s(self, field, value):\n    self.env['x.y'].sudo().search([(field, '=', value)])\n";
        assert_eq!(sinks(condition), [FlowOrigin::Param(0)]);
        let joined = "def _s(self, emails, extra):\n    d = expression.AND([[('email', 'in', emails)], extra])\n    self.env['x.y'].sudo().search(d)\n";
        assert_eq!(sinks(joined), [FlowOrigin::Param(1)]);
        let returned = "def _s(self, ids):\n    d = Domain('id', 'in', ids) | Domain(self._base())\n    self.env['x.y'].sudo().search(d)\n";
        assert_eq!(sinks(returned), [FlowOrigin::Return(0)]);
        let and_returned = "def _s(self, ids):\n    d = Domain('id', 'in', ids) & Domain(self._base())\n    self.env['x.y'].sudo().search(d)\n";
        assert_eq!(sinks(and_returned), [FlowOrigin::Return(0)]);
        let and_param = "def _s(self, emails, extra):\n    d = Domain(extra) & Domain('email', 'in', emails)\n    self.env['x.y'].sudo().search(d)\n";
        assert_eq!(sinks(and_param), [FlowOrigin::Param(1)]);
    }

    #[test]
    fn a_domain_a_call_returns_keeps_the_conditions_of_the_user_harmless() {
        let codes = |body: &str| {
            let src = format!("def f(self, t):\n    d = self._base()\n{body}\n    self.sudo().search(d)\n");
            flows_of(Params::Route(&[]), &src).0
        };
        assert!(codes("    d += [('name', 'ilike', t)]").is_empty());
        assert_eq!(codes("    d += [(t, 'ilike', 'a')]"), ["OD715"]);
    }

    #[test]
    fn only_records_under_sudo_read_a_field_by_item() {
        for body in [
            "    P.sudo(False)[t]",
            "    self.env[t].sudo(False).search([])",
            "    P.sudo()._mapping()[t]",
            "    m = P.sudo()._mapping()\n    m[t]",
            "    request.env(su=True)[t]",
            "    m = request.env(su=True)\n    m[t]",
            "    r = P.sudo().with_user(request.env.user)\n    r[t]",
            "    r = P.sudo(False)\n    r[t]",
            "    P.sudo(flag)[t]",
        ] {
            assert!(codes_of(body).is_empty(), "{body}");
        }
        for body in [
            "    r = P.sudo().browse(1)\n    r[t]",
            "    r = P.sudo().search([])\n    r.with_context(a=1)[t]",
            "    request.env(su=True)['x'][t]",
            "    M = request.env(su=True)['x']\n    M[t]",
            "    r = P.with_user(SUPERUSER_ID)\n    r[t]",
        ] {
            assert_eq!(codes_of(body), ["OD714"], "{body}");
        }
    }

    #[test]
    fn a_mapping_get_returns_a_value_whoever_picks_the_key() {
        let leaves = "    filters = {'a': [('name', '=', t)]}\n";
        assert!(
            codes_of(&format!(
                "{leaves}    filters = dict(filters)\n    P.sudo().search(filters.get(t))"
            ))
            .is_empty()
        );
        assert_eq!(
            codes_of(&format!(
                "{leaves}    filters = dict(filters)\n    P.sudo().search(filters.get('a', t))"
            )),
            ["OD715"]
        );
    }

    #[test]
    fn three_conditions_domains_or_fields_are_no_condition() {
        for call in [
            "P.sudo().search([('a', '=', 1), ('b', '=', 2), (t, '=', 3)])",
            "P.sudo().search(['|', ('a', '=', 1), t])",
            "P.sudo().search_read([], ['name', 'ref', t])",
            "P.sudo().read_group([], ['a', 'b', t], ['c'])",
            "P.sudo().search(expression.AND([[('a', '=', 1)], [('b', '=', 2)], t]))",
            "P.sudo().search([leaf_a, leaf_b, t])",
            "P.sudo().search([self._a(), b, t])",
            "P.sudo().search_read([], [f1, f2, t])",
            "P.sudo().search_read([], ['name', f, t])",
            "P.sudo().search_fetch([], field_names=['name', '=', t])",
        ] {
            assert_eq!(codes_of(&format!("    {call}")), ["OD715"], "{call}");
        }
        for call in [
            "P.sudo().search([('a', op, t)])",
            "P.sudo().search([('a', 'not =ilike', t)])",
            "P.sudo().search([(fname, '=', t)])",
            "P.sudo().search([(f'partner_id.{key}', '=', t)])",
            "P.sudo().search([(fname, op, t)])",
            "P.sudo().search([(self._rec_name, op, t)])",
            "P.sudo().search([('%s.name' % p, op, t)])",
            "P.sudo().search(Domain(fname, op, t))",
            "P.sudo().search(Domain('id' if x else 'name', op, t))",
        ] {
            assert!(codes_of(&format!("    {call}")).is_empty(), "{call}");
        }
    }

    #[test]
    fn a_returned_value_still_reports_what_the_kept_one_is_harmless_for() {
        let src = "def f(self, field, domain):\n    if field not in self._fields:\n        raise ValueError()\n    self.env.cr.execute('SELECT %s FROM t WHERE %s' % (field, self._where(domain)))\n";
        assert_eq!(flows_of(Params::Rpc, src).0, ["OD701"]);
        let src = "def f(self, name, ext):\n    open(os.path.basename(name) + self._suffix(ext))\n";
        assert_eq!(flows_of(Params::Route(&[]), src).0, ["OD707"]);
    }

    #[test]
    fn a_join_keeps_the_value_harmless_for_fewer_rules() {
        let src = "def _set(self, field, value):\n    if field not in self._fields:\n        raise ValueError()\n    self.env.cr.execute(\"UPDATE t SET %s = '%s'\" % (field, value))\n";
        let found = summary(Params::None, src);
        let sinks = found.edges.iter().filter(|e| e.target == FlowTarget::Sink("OD701"));
        assert_eq!(sinks.map(|e| e.origin).collect::<Vec<_>>(), [FlowOrigin::Param(1)]);
    }

    #[test]
    fn the_list_a_domain_join_takes_holds_domains() {
        for join in ["expression.AND", "expression.OR", "Domain.AND", "Domain.OR"] {
            let body = format!("    P.sudo().search({join}([d1, d2, t]))");
            assert_eq!(codes_of(&body), ["OD715"], "{join}");
        }
        let imported = |import: &str, body: &str| codes_after(import, &format!("    {body}"));
        for (import, join) in [
            ("from odoo.osv.expression import AND", "AND"),
            ("from odoo.osv.expression import OR", "OR"),
            ("from odoo.osv.expression import AND as all_of", "all_of"),
        ] {
            for list in ["[d1, d2, t]", "['name', op, t]"] {
                let body = format!("P.sudo().search({join}({list}))");
                assert_eq!(imported(import, &body), ["OD715"], "{import}: {body}");
            }
        }
        assert!(imported("import os", "P.sudo().search(AND(['name', op, t]))").is_empty());
        let import = "from odoo.osv.expression import AND";
        assert!(imported(import, "P.sudo().search(AND([[('name', '=', t)], d2]))").is_empty());
    }

    #[test]
    fn an_allow_list_named_like_fields_confines_every_rule() {
        let raises = "\n        raise ValueError()";
        assert!(codes_of(&format!("    if t not in allowed_fields:{raises}\n    P.sudo()[t]")).is_empty());
        assert!(
            codes_of(&format!(
                "    if t not in self._readable_fields:{raises}\n    P.sudo()[t]"
            ))
            .is_empty()
        );
        for allowed in ["allowed_fields", "self.env['x.y']._fields"] {
            let src = format!(
                "def f(self, **post):\n    if any(k not in {allowed} for k in post):{raises}\n    self.env['x.y'].sudo().create(post)\n"
            );
            assert!(scan(Params::Route(&[]), &src).is_empty(), "{allowed}");
        }
        let browsed = format!(
            "def f(self, order_id, **post):\n    order = self.env['x.y'].browse(int(order_id))\n    if any(k not in order._fields for k in post):{raises}\n    order.sudo().write(post)\n"
        );
        assert!(scan(Params::Route(&[]), &browsed).is_empty());
    }

    #[test]
    fn a_comprehension_guards_only_on_what_each_test_holds() {
        let either = "    cols = [c for c in t if c in ALLOWED or c.startswith('x_')]\n    self.env.cr.execute('SELECT %s FROM t' % ', '.join(cols))";
        assert_eq!(codes_of(either), ["OD701"]);
        assert_eq!(
            codes_of("    P.sudo().read([f for f in t if not f in allowed])"),
            ["OD714"]
        );
        assert!(codes_of("    P.sudo().read([f for f in t if f and f in ('name', 'ref')])").is_empty());
    }

    #[test]
    fn sudo_false_writes_with_the_rights_of_the_user() {
        let write = |body: &str| {
            let src = format!("def f(self, flag, **post):\n{body}.write(post)\n");
            route_codes(&src)
        };
        for body in [
            "    self.env['x.y'].sudo()",
            "    self.env['x.y'].sudo(True)",
            "    self.env['x.y'].sudo(flag=True)",
            "    self.env['x.y'].with_user(SUPERUSER_ID)",
            "    self.env['x.y'].with_user(1)",
            "    self.env['x.y'].with_user(request.env.user).sudo()",
            "    self.env['x.y'].sudo().with_user(SUPERUSER_ID)",
            "    self.env['x.y'].sudo(False).sudo()",
            "    request.env(su=True)['x.y']",
            "    request.env(user=SUPERUSER_ID)['x.y']",
            "    api.Environment(request.cr, SUPERUSER_ID, {})['x.y']",
            "    env = request.env(su=True)\n    env['x.y']",
            "    R = self.env['x.y'].sudo().with_user(SUPERUSER_ID)\n    R",
            "    R = self.env['x.y'].with_user(request.env.user)\n    R.sudo()",
            "    self.env['x.y'].sudo(SUPERUSER_ID)",
            "    self.env['x.y'].sudo(1)",
            "    self.env['x.y'].sudo(user=SUPERUSER_ID)",
            "    self.env['x.y'].sudo(SUPERUSER_ID).sudo(False)",
            "    self.env['x.y'].with_user(SUPERUSER_ID).sudo(False)",
            "    self.env['x.y'].with_user(1).sudo().sudo(False)",
            "    request.env(user=SUPERUSER_ID)['x.y'].sudo(False)",
            "    api.Environment(request.cr, SUPERUSER_ID, {})['x.y'].sudo(False)",
            "    R = self.env['x.y'].with_user(SUPERUSER_ID)\n    R.sudo(False)",
            "    R = self.env['x.y'].with_user(1).sudo(False)\n    R.sudo(False)",
        ] {
            assert_eq!(write(body), ["OD704"], "{body}");
        }
        for body in [
            "    self.env['x.y'].sudo(False)",
            "    self.env['x.y'].sudo(flag=False)",
            "    self.env['x.y'].sudo(flag)",
            "    self.env['x.y'].sudo().sudo(False)",
            "    self.env['x.y'].sudo().with_user(request.env.user)",
            "    self.env['x.y'].with_user(1).with_user(request.env.user)",
            "    self.env['x.y'].sudo().with_context(a=1).with_user(request.env.user).with_context(b=1)",
            "    request.env(su=False)['x.y']",
            "    request.env(su=True)['x.y'].with_user(request.env.user)",
            "    R = self.env['x.y'].sudo().with_user(request.env.user)\n    R",
            "    R = self.env['x.y'].sudo(False)\n    R",
            "    R = self.env['x.y'].sudo()\n    R.sudo(False)",
            "    R = self.env['x.y'].sudo()\n    R.with_user(request.env.user)",
            "    self.env['x.y'].sudo(user=request.env.user)",
            "    self.env['x.y'].sudo(request.env.user)",
            "    request.env(su=True)['x.y'].sudo(False)",
            "    self.env['x.y'].with_user(1).sudo(False).with_user(request.env.user)",
            "    R = self.env['x.y'].with_user(1)\n    R.with_user(request.env.user)",
            "    R = request.env(su=True)['x.y']\n    R.sudo(False)",
        ] {
            assert!(write(body).is_empty(), "{body}");
        }
    }

    #[test]
    fn sudo_false_keeps_uid_1_superuser_from_saas_12_5() {
        let found = |series: Option<&str>, body: &str| {
            let src = format!("def f(self, order_id, **post):\n{body}\n");
            route_codes_in(series, &src)
        };
        for (body, code) in [
            (
                "    self.env['x.y'].sudo(SUPERUSER_ID).sudo(False).write(post)",
                "OD704",
            ),
            (
                "    self.env['x.y'].sudo(user=SUPERUSER_ID).sudo(False).write(post)",
                "OD704",
            ),
            (
                "    request.env(user=SUPERUSER_ID)['x.y'].sudo(False).write(post)",
                "OD704",
            ),
            (
                "    api.Environment(request.cr, SUPERUSER_ID, {})['x.y'].sudo(False).write(post)",
                "OD704",
            ),
            (
                "    R = self.env['x.y'].sudo(SUPERUSER_ID)\n    R.sudo(False).write(post)",
                "OD704",
            ),
            (
                "    return request.env['sale.order'].browse(order_id).sudo(SUPERUSER_ID).sudo(False)",
                "OD716",
            ),
        ] {
            for series in [Some("saas~12.5"), Some("13.0"), None] {
                assert_eq!(found(series, body), [code], "{body} on {series:?}");
            }
            for series in ["12.0", "saas~12.4"] {
                assert!(found(Some(series), body).is_empty(), "{body} on {series}");
            }
        }
    }

    #[test]
    fn a_local_superuser_on_either_branch_keeps_uid_1() {
        for (first, second) in [
            ("self.env['x.y'].with_user(SUPERUSER_ID)", "self.env['x.y'].sudo()"),
            ("self.env['x.y'].sudo()", "self.env['x.y'].with_user(SUPERUSER_ID)"),
        ] {
            let src = format!(
                "def f(self, c, **post):\n    if c:\n        R = {first}\n    else:\n        R = {second}\n    R.sudo(False).write(post)\n"
            );
            assert_eq!(route_codes(&src), ["OD704"], "{first}");
        }
    }

    #[test]
    fn a_sudo_sink_names_the_call_that_makes_it_superuser() {
        for (body, callee) in [
            ("self.env['x.y'].sudo().write(post)", "sudo().write()"),
            (
                "self.env['x.y'].with_user(SUPERUSER_ID).write(post)",
                "with_user().write()",
            ),
            (
                "self.env['x.y'].with_user(1).sudo(False).write(post)",
                "with_user().write()",
            ),
            (
                "self.env['x.y'].with_user(1).with_user(u).sudo().write(post)",
                "sudo().write()",
            ),
            ("request.env(su=True)['x.y'].create(post)", "env().create()"),
            (
                "api.Environment(request.cr, SUPERUSER_ID, {})['x.y'].write(post)",
                "Environment().write()",
            ),
            (
                "R = self.env['x.y'].with_user(SUPERUSER_ID)\n    R.write(post)",
                "sudo().write()",
            ),
            ("request.env(su=True)[t].search([])", "env().search()"),
            ("self.env['x.y'].with_user(1)[t]", "with_user()[...]"),
            (
                "request.env(su=True)['ir.attachment'].create({'raw': t, 'public': True})",
                "env().create()",
            ),
        ] {
            let src = format!("def f(self, t, u, **post):\n    {body}\n");
            let found = scan_in(&TaintTable::default(), Some("18.0"), Params::Route(&[]), &src);
            assert!(
                found.iter().any(|f| f.contains(&format!(" {callee}"))),
                "{body}: {found:?}"
            );
        }
    }

    /// The codes of a route body where `order_id` and `token` are its parameters and `O` the
    /// orders read as superuser.
    fn returned(params: Params, body: &str) -> Vec<String> {
        let src = format!("def f(self, order_id, token):\n    O = request.env['sale.order'].sudo()\n{body}\n");
        codes_from(&scan(params, &src))
    }

    #[test]
    fn a_route_returning_records_the_user_picks_as_superuser_reports_them() {
        for body in [
            "    return O.browse(int(order_id)).name",
            "    order = request.env['sale.order'].browse(order_id).sudo()\n    return order",
            "    order = O.browse([int(i) for i in order_id.split(',')])\n    return request.render('t', {'order': order})",
            "    order = O.browse(map(int, order_id.split(',')))\n    return order.read(['name'])",
            "    order_id = int(order_id)\n    order = O.browse(order_id).exists()\n    return order.partner_id",
            "    values = {}\n    values['order'] = O.browse(order_id)\n    return request.render('t', values)",
            "    values = {}\n    values.update(order=O.browse(order_id))\n    return request.render('t', values)",
            "    order = O.browse(order_id)\n    order.sudo().check_access('read')\n    return order",
            "    order = O.browse(order_id)\n    order.with_user(request.env.user).sudo().check_access('read')\n    return order",
            "    order = O.browse(order_id)\n    if not order.exists():\n        raise NotFound()\n    return order",
            "    order = O.browse(order_id)\n    if order:\n        return order.name",
            "    self._document_check_access('sale.order', order_id, token)\n    order_id = token\n    return O.browse(order_id)",
            "    self._document_check_access('sale.order', order_id, token)\n    return O.browse([int(order_id), int(token)])",
            "    order = O.browse(order_id)\n    if not order.sudo().exists():\n        raise NotFound()\n    return order",
            "    order = O.browse(order_id)\n    if order.id:\n        return order",
            "    values = {'order': O.browse(order_id)}\n    if values.get('error'):\n        return values",
            "    values = {'order': O.browse(order_id)}\n    if values.get('error') == 'x':\n        return values",
            "    order = O.browse(order_id)\n    if order.with_context(lang='en_US').name:\n        return order",
            "    self._checkout_form_save(order_id)\n    return O.browse(order_id)",
            "    order_id = int(order_id) if order_id else False\n    return O.browse(order_id)",
            "    return O.browse(int(order_id) if order_id else False)",
            "    order_sudo = O.browse(int(order_id))\n    order_sudo.partner_id.check_access('read')\n    return order_sudo",
            "    order = request.env['sale.order'].with_user(SUPERUSER_ID).browse(order_id)\n    return order",
            "    return request.env(su=True)['sale.order'].browse(order_id)",
            "    env = request.env(su=True)\n    return env['sale.order'].browse(order_id)",
            "    return request.env['sale.order'].browse(order_id).with_user(SUPERUSER_ID)",
            "    order = O.browse(order_id)\n    order.with_user(SUPERUSER_ID).check_access('read')\n    return order",
            "    order = O.browse(order_id)\n    order.with_user(1).sudo(False).check_access('read')\n    return order",
            "    return request.env['sale.order'].browse(order_id).with_user(SUPERUSER_ID).sudo(False)",
            "    return request.env['sale.order'].with_user(1).browse(order_id).sudo(False)",
            "    order = O.browse(order_id)\n    if not consteq(order.access_token, token):\n        return request.render('t', {'order': order})",
            "    order = O.browse(order_id)\n    if not token:\n        raise NotFound()\n    elif not consteq(order.access_token, token):\n        return order.name",
            "    order = O.browse(order_id)\n    if consteq(order.access_token, token):\n        return order.name\n    return order",
            "    order = O.browse(order_id)\n    if consteq(order.access_token, token):\n        pass\n    else:\n        return order",
            "    order = O.browse(order_id)\n    if consteq(order.access_token, token) == True:\n        return order",
            "    order = O.browse(order_id)\n    if not order.sudo(False).has_access('read'):\n        return order.name",
            "    order = O.browse(order_id)\n    assert not consteq(order.access_token, token)\n    return order",
            "    order = O.browse(order_id)\n    if not token or not consteq(order.access_token, token):\n        return request.render('t', {'order': order})",
            "    order = O.browse(order_id)\n    if not (token and consteq(order.access_token, token)):\n        return order",
            "    order = O.browse(order_id)\n    if token and consteq(order.access_token, token):\n        return order.name\n    return order",
        ] {
            assert_eq!(returned(Params::Route(&[]), body), ["OD716"], "{body}");
        }
        let token_in_kw = "def f(self, order_id, **kw):\n    self._document_check_access('sale.order', order_id, kw.get('access_token'))\n    return request.env['res.partner'].sudo().browse(int(kw['partner_id']))\n";
        assert_eq!(route_codes(token_in_kw), ["OD716"]);
        // The check of one member of a holder vouches for that member only.
        let holder = |holds: &str| {
            format!(
                "def f(self, order_id, partner_id, token):\n    O = request.env['sale.order'].sudo()\n    order = O.browse(order_id)\n    partner = O.browse(int(partner_id))\n    {holds}\n    if not consteq(order.access_token, token):\n        raise NotFound()\n    return partner.name\n"
            )
        };
        assert_eq!(
            route_codes(&holder("values = {'order': order, 'partner': partner}")),
            ["OD716"]
        );
        assert_eq!(route_codes(&holder("both = [order, partner]")), ["OD716"]);
        let whole = holder("values = {'order': order, 'partner': partner}\n    self._check_values(values)");
        assert!(route_codes(&whole).is_empty());
        let compared_to_kw = "def f(self, order_id, **kw):\n    order = request.env['sale.order'].sudo().browse(order_id)\n    if not consteq(order.access_token, kw['token']):\n        raise NotFound()\n    return request.env['res.partner'].sudo().browse(int(kw['partner_id']))\n";
        assert_eq!(route_codes(compared_to_kw), ["OD716"]);
        let found = scan(
            Params::Route(&[]),
            "def f(self, order_id):\n    order = request.env['sale.order'].sudo().browse(order_id)\n    return order.name\n",
        );
        assert_eq!(
            found,
            [
                "OD716 route parameter 'order_id' reaches the id of browse() through 'order', whose records the route returns read as superuser"
            ]
        );
    }

    #[test]
    fn a_check_vouches_for_the_records_a_route_returns() {
        for body in [
            "    order = O.browse(order_id)\n    order = self._document_check_access('sale.order', order.id, token)\n    return order",
            "    self._document_check_access('sale.order', order_id, token)\n    return O.browse(order_id)",
            "    order = O.browse(int(order_id))\n    if not consteq(order.access_token, token):\n        raise NotFound()\n    return order",
            "    order = O.browse(order_id)\n    secret = order._token()\n    if not hmac.compare_digest(secret, token):\n        raise NotFound()\n    return order",
            "    if not payment_utils.check_access_token(token, order_id):\n        raise NotFound()\n    return O.browse(order_id)",
            "    order = O.browse(order_id)\n    self._check_order(order, token)\n    return order",
            "    request.env['sale.order'].browse(order_id).check_access('read')\n    return O.browse(order_id)",
            "    order = request.env['sale.order'].browse(order_id)\n    order.check_access_rule('read')\n    return order.sudo()",
            "    order = O.browse(order_id)\n    if not request.env['sale.order'].browse(order_id).has_access('read'):\n        raise NotFound()\n    return order",
            "    order = O.browse(order_id)\n    if order.partner_id != request.env.user.partner_id:\n        raise NotFound()\n    return order",
            "    order = O.browse(order_id)\n    if order.state == 'sale':\n        return order.name",
            "    order = O.browse(order_id)\n    if token != order._access_token():\n        raise NotFound()\n    return order",
            "    order = O.browse(order_id)\n    if order.id not in allowed_ids:\n        raise NotFound()\n    return order",
            "    order_sudo = O.browse(int(order_id))\n    order_sudo.with_user(request.env.user).check_access('read')\n    return request.render('t', {'order': order_sudo})",
            "    order = O.browse(order_id)\n    assert order in request.env.user.order_ids\n    return order",
            "    if int(order_id) not in request.env.user.order_ids.ids:\n        raise NotFound()\n    return O.browse(int(order_id))",
            "    if order_id:\n        if not payment_utils.check_access_token(token, order_id):\n            raise NotFound()\n    return O.browse(order_id)",
            "    self._document_check_access('sale.order', order_id, token)\n    oid = int(order_id)\n    return O.browse(oid)",
            "    if not order_id in allowed:\n        raise NotFound()\n    return O.browse(order_id)",
            "    order_id = int(order_id)\n    if order_id not in allowed:\n        raise NotFound()\n    return O.browse(order_id)",
            "    order = O.browse(order_id)\n    if not order.sudo(False).has_access('read'):\n        raise NotFound()\n    return order",
            "    order_sudo = O.browse(int(order_id))\n    order = order_sudo.with_user(request.env.user)\n    order.check_access('read')\n    return request.render('t', {'order': order_sudo})",
            "    order = O.browse(order_id)\n    if consteq(order.access_token, token):\n        return order",
            "    order = O.browse(order_id)\n    if token and consteq(order.access_token, token):\n        return order",
            "    order = O.browse(order_id)\n    if not (token and consteq(order.access_token, token)):\n        raise NotFound()\n    return order",
            "    order = O.browse(order_id)\n    values = {'order': order}\n    if not consteq(order.access_token, token):\n        raise NotFound()\n    return request.render('t', values)",
            "    order = O.browse(order_id)\n    mine = order\n    if not consteq(order.access_token, token):\n        raise NotFound()\n    return mine",
            "    order = O.browse(order_id)\n    if not consteq(order.access_token, token):\n        raise NotFound()\n    return O.browse(order_id)",
            "    order = O.browse(order_id)\n    ok = consteq(order.access_token, token)\n    if not ok:\n        raise NotFound()\n    return order",
            "    order = O.browse(order_id)\n    assert consteq(order.access_token, token)\n    return order",
            "    order = O.browse(order_id)\n    if not token or not consteq(order.access_token, token):\n        raise NotFound()\n    return order",
            "    order = request.env['sale.order'].browse(order_id)\n    if not order.has_access('read') and not consteq(order.sudo().access_token, token):\n        raise NotFound()\n    return request.render('t', {'order': order.sudo()})",
            "    order = request.env['sale.order'].browse(order_id).exists()\n    if not order or (not order.has_access('read') and not order._owned(token)):\n        raise NotFound()\n    return order.sudo()",
            "    order = O.browse(order_id)\n    if consteq(order.access_token, token) or token == 'demo':\n        return order",
        ] {
            assert!(returned(Params::Route(&[]), body).is_empty(), "{body}");
        }
    }

    #[test]
    fn only_routes_returning_records_read_as_superuser_report_them() {
        for (params, body) in [
            (Params::Rpc, "    return O.browse(order_id)"),
            (Params::None, "    return O.browse(order_id)"),
            (Params::Route(&["order_id"]), "    return O.browse(order_id.id)"),
            (
                Params::Route(&[]),
                "    return request.env['sale.order'].browse(order_id)",
            ),
            (
                Params::Route(&[]),
                "    return O.browse(order_id).with_user(request.env.user)",
            ),
            (Params::Route(&[]), "    return O.browse(order_id).sudo(False)"),
            (Params::Route(&[]), "    return O.browse(order_id).sudo(token)"),
            (
                Params::Route(&[]),
                "    return O.browse(order_id).sudo().with_user(request.env.user)",
            ),
            (
                Params::Route(&[]),
                "    return request.env['sale.order'].with_user(1).with_user(request.env.user).browse(order_id)",
            ),
            (Params::Route(&[]), "    return O.browse(order_id).get_portal_url()"),
            (Params::Route(&[]), "    return len(O.browse(order_id))"),
            (
                Params::Route(&[]),
                "    return request.env['sale.order'].browse(order_id).name",
            ),
            (
                Params::Route(&[]),
                "    order = O.browse(order_id)\n    order = O.browse(1)\n    return order",
            ),
        ] {
            assert!(returned(params, body).is_empty(), "{body}");
        }
    }

    #[test]
    fn records_a_called_method_picks_are_left_to_its_caller() {
        let src = "def f(self, order_id):\n    oid = self._helper(order_id)\n    return request.env['sale.order'].sudo().browse(oid)\n";
        let route = summary(Params::Route(&[]), src);
        assert!(!route.edges.iter().any(|e| e.target == FlowTarget::Sink("OD716")));
    }

    #[test]
    fn an_id_read_from_records_reads_them_as_they_were() {
        let rebound = "    recs = self.env['x.y']\n    recs = self.env['x.y'].sudo().browse(recs.search(t).ids)";
        assert!(codes_of(rebound).is_empty());
    }

    #[test]
    fn records_leave_the_other_flows_as_they_were() {
        let guarded = "    if not consteq(t, 'x'):\n        raise NotFound()\n    self.env.cr.execute('SELECT %s' % t)";
        assert_eq!(codes_of(guarded), ["OD701"]);
        let arm = "    r = P.sudo().browse(t)\n    if r.name:\n        Markup(t)";
        assert_eq!(codes_of(arm), ["OD703"]);
        let update =
            "def f(self, **post):\n    vals = {}\n    vals.update(post)\n    self.env['x.y'].sudo().create(vals)\n";
        assert_eq!(route_codes(update), ["OD704"]);
        assert_eq!(
            codes_of(
                "    r = P.sudo().browse(t)\n    response = request.make_response('')\n    response.headers['X-A'] = t"
            ),
            ["OD711"]
        );
    }

    /// The codes of a route of a module of `series`, where `upload` and `post` are its parameters
    /// and `A` the attachments under `sudo()`.
    fn stored(series: Option<&str>, body: &str) -> Vec<String> {
        let src = format!("def f(self, upload, **post):\n    A = request.env['ir.attachment'].sudo()\n{body}\n");
        route_codes_in(series, &src)
    }

    #[test]
    fn user_content_in_a_public_attachment_under_sudo_is_reported() {
        for body in [
            "    A.create({'raw': upload.read(), 'public': True})",
            "    request.env['ir.attachment'].sudo().create(vals_list={'db_datas': upload, 'public': True})",
            "    vals = {'name': 'a', 'raw': upload}\n    vals['public'] = True\n    A.create(vals)",
            "    vals = {'name': 'a', 'public': True}\n    vals['raw'] = upload\n    A.create(vals)",
            "    A.create(dict(raw=upload, public=True))",
            "    A.create(dict({'raw': upload}, public=True))",
            "    A.create([{'raw': upload, 'public': True}])",
            "    A.create(({'name': 'a'}, {'raw': upload, 'public': True}))",
            "    A.create({**{'raw': upload}, 'public': True})",
            "    A.create({'datas': base64.b64encode(upload.read()), 'public': True})",
            "    A._upload_file(upload, {'public': True})",
            "    A._upload_file(file=upload, create_vals={'name': 'a', 'public': True})",
            "    vals = {'raw': upload}\n    if post:\n        vals = {'raw': upload, 'public': True}\n    A.create(vals)",
            "    public = {'raw': upload, 'public': True}\n    vals = public\n    A.create(vals)",
            "    request.env['ir.attachment'].with_user(1).create({'raw': upload, 'public': True})",
            "    request.env['ir.attachment'].sudo().with_user(SUPERUSER_ID).create({'raw': upload, 'public': True})",
            "    request.env(su=True)['ir.attachment'].create({'raw': upload, 'public': True})",
            "    request.env['ir.attachment'].with_user(1).sudo(False).create({'raw': upload, 'public': True})",
            "    B = request.env['ir.attachment'].with_user(1)\n    B.sudo(False).create({'raw': upload, 'public': True})",
            "    vals = {'raw': upload}\n    vals |= {'public': True}\n    A.create(vals)",
            "    A.create([{'raw': b'x', 'public': True}, {'raw': upload, 'public': True}])",
            "    vals = {'raw': upload, 'db_datas': b'x'}\n    vals['db_datas'] = b'y'\n    vals['public'] = True\n    A.create(vals)",
            "    extra = {'public': True}\n    if post.get('logo'):\n        extra['raw'] = b'logo'\n    A.create({'raw': upload.read(), **extra})",
            "    extra = {'public': True}\n    for _ in range(2):\n        extra['raw'] = b'logo'\n    A.create({'raw': upload.read(), **extra})",
            "    vals = {'raw': upload}\n    extra = {'public': True}\n    if post.get('logo'):\n        extra['raw'] = b'logo'\n    vals |= extra\n    A.create(vals)",
            "    vals = {'raw': upload, 'public': True}\n    extra = self._defaults()\n    vals |= extra\n    A.create(vals)",
            "    vals = {'raw': upload}\n    data = vals.copy()\n    data['public'] = True\n    A.create(data)",
            "    extra = self._defaults()\n    if post.get('logo'):\n        extra['raw'] = b'logo'\n    A.create({'raw': upload.read(), **extra, 'public': True})",
            "    extra = self._defaults()\n    if post.get('private'):\n        extra['public'] = False\n    A.create({'raw': upload.read(), 'public': True, **extra})",
            "    extra = {'raw': b'logo'}\n    if post.get('custom'):\n        extra = self._custom()\n    A.create({'raw': upload.read(), **extra, 'public': True})",
            "    vals = {'raw': upload.read(), 'public': True}\n    extra = self._defaults()\n    if post.get('logo'):\n        extra['raw'] = b'logo'\n    vals |= extra\n    A.create(vals)",
        ] {
            assert_eq!(stored(Some("18.0"), body), ["OD717"], "{body}");
        }
        let found = scan_in(
            &TaintTable::default(),
            None,
            Params::Route(&[]),
            "def f(self, upload):\n    vals = {'raw': upload, 'public': True}\n    request.env['ir.attachment'].sudo().create(vals)\n",
        );
        assert_eq!(
            found,
            [
                "OD717 route parameter 'upload' reaches the content of sudo().create() through 'vals', which public=True lets anyone download"
            ]
        );
    }

    #[test]
    fn a_private_attachment_or_one_created_with_a_user_rights_is_not_reported() {
        for body in [
            "    request.env['ir.attachment'].create({'raw': upload, 'public': True})",
            "    A.with_user(request.env.user).create({'raw': upload, 'public': True})",
            "    request.env['ir.attachment'].sudo().with_user(request.env.user).create({'raw': upload, 'public': True})",
            "    request.env['ir.attachment'].sudo(flag).create({'raw': upload, 'public': True})",
            "    request.env['ir.attachment'].sudo(False).create({'raw': upload, 'public': True})",
            "    B = request.env['ir.attachment'].sudo().with_user(request.env.user)\n    B.create({'raw': upload, 'public': True})",
            "    B = request.env['ir.attachment'].sudo(False)\n    B.create({'raw': upload, 'public': True})",
            "    B = request.env(su=True)['ir.attachment'].sudo(False)\n    B.create({'raw': upload, 'public': True})",
            "    request.env['res.partner'].sudo().create({'raw': upload, 'public': True})",
            "    A.create({'raw': upload})",
            "    A.create({'raw': upload, 'public': bool(post)})",
            "    A.create({'raw': upload, 'public': post.get('public')})",
            "    A.create({'raw': upload, 'public': False})",
            "    vals = {'raw': upload, 'public': True}\n    vals['public'] = post.get('public')\n    A.create(vals)",
            "    vals = {'raw': upload, 'public': True}\n    vals = self._prepare(upload)\n    A.create(vals)",
            "    A.create({'name': upload.filename, 'res_id': int(post['id']), 'raw': b'x', 'public': True})",
            "    A.create({'name': upload, 'mimetype': upload, 'public': True})",
            "    A._upload_file(upload, {'name': 'a'})",
            "    A.write({'raw': upload, 'public': True})",
            "    A.create([{'raw': b'logo', 'public': True}, {'raw': upload}])",
            "    A.create([{'raw': upload}, {'raw': b'logo', 'public': True}])",
            "    vals = {'raw': upload, 'public': True}\n    vals['raw'] = b'x'\n    A.create(vals)",
            "    A.create(dict({'raw': upload}, raw=b'x', public=True))",
            "    A.create({'raw': upload, **{'raw': b'x'}, 'public': True})",
            "    vals = {'raw': upload}\n    vals |= {'raw': b'x', 'public': True}\n    A.create(vals)",
            "    vals = {'raw': upload, 'public': True}\n    vals |= {'public': post.get('public')}\n    A.create(vals)",
            "    vals = {'name': post.get('name'), 'raw': b'pdf'}\n    data = vals.copy()\n    data['public'] = True\n    A.create(data)",
            "    name = post.get('name')\n    name['public'] = True\n    A.create(name)",
        ] {
            assert!(stored(Some("18.0"), body).is_empty(), "{body}");
        }
    }

    #[test]
    fn datas_holds_the_content_before_saas_19_3() {
        let datas = "    A.create({'datas': upload, 'public': True})";
        for series in [Some("16.0"), Some("18.0"), Some("saas~19.2"), None] {
            assert_eq!(stored(series, datas), ["OD717"], "{series:?}");
        }
        for series in ["saas~19.3", "20.0"] {
            assert!(stored(Some(series), datas).is_empty(), "{series}");
            assert_eq!(
                stored(Some(series), "    A.create({'raw': upload, 'public': True})"),
                ["OD717"],
                "{series}"
            );
        }
    }

    #[test]
    fn a_public_attachment_leaves_the_mass_assignment_rule_as_it_was() {
        assert_eq!(
            stored(None, "    A.create({**post, 'public': True})"),
            ["OD717", "OD704"]
        );
        assert_eq!(stored(None, "    A.create(post)"), ["OD704"]);
        assert_eq!(
            stored(None, "    A.create({'raw': post['data'], 'public': True})"),
            ["OD717"]
        );
        assert_eq!(
            stored(None, "    A.create(dict(post, public=True))"),
            ["OD717", "OD704"]
        );
        assert_eq!(
            stored(
                None,
                "    data = post.copy()\n    data['public'] = True\n    A.create(data)"
            ),
            ["OD717", "OD704"]
        );
        assert_eq!(
            stored(None, "    post['public'] = True\n    A.create(post)"),
            ["OD717", "OD704"]
        );
        assert_eq!(
            stored(None, "    A.create({**post, 'raw': b'x', 'public': True})"),
            ["OD717", "OD704"]
        );
        assert_eq!(
            stored(
                None,
                "    data = request.params\n    data['public'] = True\n    A.create(data)"
            ),
            ["OD717", "OD704"]
        );
        // An augmented assignment binds `post` as a value, which drops OD704.
        let merged = stored(None, "    post |= {'public': True}\n    A.create(post)");
        assert!(merged.iter().any(|c| c == "OD717"), "{merged:?}");
    }

    #[test]
    fn a_helper_storing_its_parameter_publicly_keeps_the_edge_for_its_callers() {
        let helper = summary(
            Params::None,
            "def _store(self, data):\n    self.env['ir.attachment'].sudo().create({'raw': data, 'public': True})\n",
        );
        let targets: Vec<_> = helper.edges.iter().map(|e| (e.origin, e.target.clone())).collect();
        assert_eq!(targets, [(FlowOrigin::Param(0), FlowTarget::Sink("OD717"))]);
        // Its caller picks the keys, which may set the content to the code's own.
        for publish in ["vals['public'] = True", "vals |= {'public': True}"] {
            let src =
                format!("def _store(self, vals):\n    {publish}\n    self.env['ir.attachment'].sudo().create(vals)\n");
            let edges = model_method(Params::None, &src).1.map(|s| s.edges).unwrap_or_default();
            assert!(
                !edges.iter().any(|e| e.target == FlowTarget::Sink("OD717")),
                "{publish}"
            );
            // A public method's own RPC argument is the user's mapping, so it reports there.
            let (found, _) = model_method(Params::Rpc, &src.replace("_store", "store"));
            assert!(found.iter().any(|d| d.code == "OD717"), "RPC {publish}");
        }
    }

    /// A sink of each rule but OD704, which needs a mapping, receiving `#`.
    const SINK_OF: &[(&str, &str)] = &[
        ("OD701", "self.env.cr.execute(#)"),
        ("OD702", "safe_eval(#)"),
        ("OD703", "Markup(#)"),
        ("OD705", "request.redirect(#, local=False)"),
        ("OD706", "requests.get(#, timeout=5)"),
        ("OD707", "open(#)"),
        ("OD708", "os.system(#)"),
        ("OD709", "pickle.loads(#)"),
        ("OD710", "etree.fromstring(#, etree.XMLParser(resolve_entities=True))"),
        ("OD711", "request.make_response('', [('X-Name', #)])"),
        ("OD712", "self.env[#].sudo().search([])"),
        ("OD713", "getattr(self.env['res.partner'], #)()"),
        ("OD714", "self.env['res.partner'].sudo()[#]"),
        ("OD715", "self.env['res.partner'].sudo().search(#)"),
        ("OD716", "return self.env['res.partner'].sudo().browse(#)"),
        (
            "OD717",
            "self.env['ir.attachment'].sudo().create({'raw': #, 'public': True})",
        ),
    ];

    /// The codes of a route of parameter `t` that runs `before`, then `sink` on `value`.
    fn sunk(before: &str, sink: &str, value: &str) -> Vec<String> {
        route_codes(&format!("def f(self, t):\n    {before}{}\n", sink.replace('#', value)))
    }

    /// The codes of `src`, a route whose parameters are all user data.
    fn route_codes(src: &str) -> Vec<String> {
        codes_from(&scan(Params::Route(&[]), src))
    }

    /// The codes of `route_codes(src)` in a module of `series`.
    fn route_codes_in(series: Option<&str>, src: &str) -> Vec<String> {
        codes_from(&scan_in(&TaintTable::default(), series, Params::Route(&[]), src))
    }

    #[test]
    fn reads_the_json_request_of_the_dispatcher() {
        let src = "def f(self):\n    data = request.dispatcher.jsonrequest\n    self.env.cr.execute(f\"SELECT {data['x']}\")\n";
        assert_eq!(
            scan(Params::None, src),
            ["OD701 request.dispatcher.jsonrequest reaches the query of cr.execute() through 'data'"]
        );
    }

    #[test]
    fn the_name_of_a_file_picked_by_its_field_is_no_user_data() {
        let get = "f = request.httprequest.files.get('ufile')\n    ";
        let loop_over = |files: &str| format!("for f in request.httprequest.files.{files}:\n        ");
        let item = "f = request.httprequest.files['ufile']\n    ";
        for (code, sink) in SINK_OF {
            for (before, value) in [
                (get, "getattr(f, 'name')"),
                (item, "f.name"),
                (&loop_over("getlist('ufile')"), "getattr(f, 'name', None)"),
                ("", "Manifest.for_addon(t).path"),
            ] {
                assert!(sunk(before, sink, value).is_empty(), "{code}: {before}{value}");
            }
            for (before, value) in [
                (get, "getattr(f, 'filename')"),
                (item, "f.filename"),
                (&loop_over("getlist(t)"), "f.name"),
                (&loop_over("values()"), "f.name"),
                (&loop_over("getlist('ufile')"), "f.stream.name"),
                (&loop_over("getlist('ufile')"), "copy.copy(f).name"),
                ("", "t.name"),
                (&format!("{get}g = f or t\n    "), "g.name"),
            ] {
                assert_eq!(sunk(before, sink, value), [*code], "{code}: {before}{value}");
            }
        }
        let src = format!("def f(self):\n    {}open(f.name)\n", loop_over("values()"));
        assert_eq!(
            scan(Params::Route(&[]), &src),
            [
                "OD707 request.httprequest.files reaches the path of open() through 'f', so the user picks the file the server opens"
            ]
        );
    }

    /// The findings of a controller's route of parameter `t` that runs `body`, where `ROOT` is a
    /// module's constant.
    fn opened(body: &str) -> Vec<String> {
        let src = format!("def f(self, t):\n{body}\n");
        let parsed = parse_module(&src).expect("valid Python");
        let Some(Stmt::FunctionDef(func)) = parsed.syntax().body.first() else {
            panic!("no function");
        };
        let owner = Some(Receiver::Class("Main".to_owned()));
        let loc = Locator::new(&src);
        let found = function(
            func,
            Params::Route(&[]),
            owner,
            None,
            &NAMES,
            &loc,
            &TaintTable::default(),
        )
        .0;
        let message = |d: RawDiag| format!("{} {}", d.code, d.message.split(';').next().unwrap_or_default());
        found.into_iter().map(message).collect()
    }

    #[test]
    fn a_normalized_path_kept_under_a_prefix_the_code_picks_is_not_reported() {
        let joined = "os.path.normpath(os.path.join(ROOT, t))";
        for body in [
            "    p = os.path.realpath(os.path.abspath(os.path.join(ROOT, t)))\n    if p.startswith(ROOT):\n        open(p)".to_owned(),
            "    p = os.path.join(ROOT, t)\n    p = os.path.normpath(p)\n    if p.startswith('/srv/docs'):\n        return open(p)".to_owned(),
            format!("    p = {joined}\n    if not p.startswith(ROOT):\n        raise ValueError()\n    open(p)"),
            format!("    p = {joined}\n    if t:\n        return\n    if not p.startswith(ROOT) or not ROOT:\n        raise ValueError()\n    open(p)"),
            format!("    p = {joined}\n    assert p.startswith((ROOT, '/srv/docs'))\n    open(p)"),
            format!("    p = {joined}\n    q = p\n    if ROOT and q.startswith(ROOT):\n        open(q)"),
            format!("    root = os.path.join(ROOT, 'static', '')\n    p = {joined}\n    if p.startswith(root):\n        open(p)"),
            format!("    if ROOT:\n        p = {joined}\n    else:\n        p = os.path.normpath(os.path.join('/srv', t))\n    if p.startswith(ROOT):\n        open(p)"),
            format!("    for t in t:\n        p = {joined}\n        if not p.startswith(ROOT):\n            continue\n        open(p)"),
            format!("    p = {joined}\n    return [open(p) for n in t if p.startswith(ROOT)]"),
        ] {
            assert!(opened(&body).is_empty(), "{body}");
        }
        let unchecked = opened("    os.path.join(ROOT, t)");
        assert_eq!(
            unchecked,
            [
                "OD707 route parameter 't' reaches the path of os.path.join(), so the user picks the file the server opens"
            ]
        );
        for body in [
            "    p = os.path.join(ROOT, t)\n    if p.startswith(ROOT):\n        open(p)".to_owned(),
            format!("    p = {joined}\n    if p.startswith(t):\n        open(p)"),
            format!("    base = self.get_base_url()\n    p = {joined}\n    if p.startswith(base):\n        open(p)"),
            format!("    p = {joined}\n    p = p + '/..'\n    if p.startswith(ROOT):\n        open(p)"),
            format!("    p = {joined}\n    if p.startswith(ROOT) or t:\n        open(p)"),
            format!("    p = {joined}\n    open(p)"),
            format!("    p = q = {joined}\n    if p.startswith(ROOT):\n        open(p)"),
            format!("    p = {joined}\n    if p.startswith(ROOT):\n        open(p)\n    open(p)"),
            format!("    p = {joined}\n    if p.startswith('/etc'):\n        raise ValueError()\n    open(p)"),
            format!("    p = {joined}\n    if t:\n        return open(p)\n    if not p.startswith(ROOT):\n        raise ValueError()\n    open(p)"),
            format!("    p = {joined}\n    data = open(p).read()\n    if not p.startswith(ROOT):\n        raise ValueError()\n    return data"),
            "    raw = os.path.join(ROOT, t)\n    open(raw)\n    p = os.path.normpath(raw)\n    if p.startswith(ROOT):\n        open(p)".to_owned(),
            "    raw = os.path.join(ROOT, t)\n    p = os.path.normpath(raw)\n    if p.startswith(ROOT):\n        open(raw)".to_owned(),
            format!("    p = {joined}\n    if p.startswith('/'):\n        open(p)"),
            format!("    p = {joined}\n    if p.startswith(('/srv', '')):\n        open(p)"),
            format!("    p = {joined}\n    if p.startswith(os.sep):\n        open(p)"),
            format!("    p = {joined}\n    [n for n in t if p.startswith(ROOT)]\n    open(p)"),
        ] {
            assert_eq!(opened(&body), unchecked, "{body}");
        }
        for body in [
            format!("    root = os.path.join(ROOT, t)\n    p = {joined}\n    if p.startswith(root):\n        open(p)"),
            format!(
                "    if ROOT:\n        p = {joined}\n    else:\n        p = os.path.join(ROOT, t)\n    if p.startswith(ROOT):\n        open(p)"
            ),
        ] {
            assert_eq!(opened(&body), [unchecked[0].clone(), unchecked[0].clone()], "{body}");
        }
    }

    /// The findings and the summary of the first function of `src`, a method of `x.y`, as codes
    /// and as the origins of the edges to each target.
    fn flows_of(params: Params, src: &str) -> (Vec<&'static str>, Vec<(FlowTarget, FlowOrigin)>) {
        let (diags, summary) = model_method(params, src);
        let edges = summary.map_or_else(Vec::new, |s| {
            s.edges.iter().map(|e| (e.target.clone(), e.origin)).collect()
        });
        (diags.iter().map(|d| d.code).collect(), edges)
    }

    /// The origins of the edges to `target`, sorted.
    fn origins_to(edges: &[(FlowTarget, FlowOrigin)], target: &FlowTarget) -> Vec<String> {
        origins_where(edges, |t| t == target)
    }

    fn origins_where(edges: &[(FlowTarget, FlowOrigin)], target: impl Fn(&FlowTarget) -> bool) -> Vec<String> {
        let mut found: Vec<String> = edges
            .iter()
            .filter(|(t, _)| target(t))
            .map(|(_, o)| format!("{o:?}"))
            .collect();
        found.sort();
        found
    }

    #[test]
    fn a_join_keeps_an_edge_for_each_origin() {
        const SOURCE: &str = "request.params.get('x')";
        const CALL: &str = "self._x()";
        // Each pair of values and the sorted origins a join of them returns; a parameter and an
        // RPC argument never share a function.
        let pairs: &[(Params, &str, &str, &[&str])] = &[
            (Params::None, SOURCE, SOURCE, &["Source"]),
            (Params::None, SOURCE, "a", &["Param(0)", "Source"]),
            (Params::Rpc, SOURCE, "a", &["Rpc(0)", "Source"]),
            (Params::None, SOURCE, CALL, &["Return(0)", "Source"]),
            (Params::None, "a", "b", &["Param(0)", "Param(1)"]),
            (Params::None, "a", CALL, &["Param(0)", "Return(0)"]),
            (Params::Rpc, "a", "b", &["Rpc(0)", "Rpc(1)"]),
            (Params::Rpc, "a", CALL, &["Return(0)", "Rpc(0)"]),
            (Params::None, CALL, "self._y()", &["Return(0)", "Return(1)"]),
        ];
        for &(params, x, y, returned) in pairs {
            for (first, second) in [(x, y), (y, x)] {
                let src = format!(
                    "def _f(self, a, b):\n    v = {first} + {second}\n    Markup(v)\n    self._z(v)\n    return v\n"
                );
                let (found, edges) = flows_of(params, &src);
                assert_eq!(origins_to(&edges, &FlowTarget::Return), returned, "{src}");
                let passed = origins_where(&edges, |t| matches!(t, FlowTarget::Arg(..)));
                assert_eq!(passed, returned, "{src}");
                // A value the user sets is a finding, and keeps no edge to the sink.
                let user = returned.iter().any(|o| *o == "Source" || o.starts_with("Rpc"));
                let (expected, sunk): (&[&str], &[&str]) = if user { (&["OD703"], &[]) } else { (&[], returned) };
                assert_eq!(found, expected, "{src}");
                assert_eq!(origins_to(&edges, &FlowTarget::Sink("OD703")), sunk, "{src}");
            }
        }
    }

    #[test]
    fn each_rule_follows_each_origin_of_a_joined_value() {
        let sudo_write = ("OD704", "self.env['res.partner'].sudo().write(#)");
        for &(code, sink) in SINK_OF.iter().chain([&sudo_write]) {
            for value in ["a or self._x()", "self._x() or a"] {
                let src = format!("def _f(self, a):\n    {}\n", sink.replace('#', value));
                let (found, edges) = flows_of(Params::None, &src);
                assert!(found.is_empty(), "{code}: {value}");
                // A route alone returns the records of OD716.
                let sunk: &[&str] = if code == "OD716" {
                    &[]
                } else {
                    &["Param(0)", "Return(0)"]
                };
                assert_eq!(origins_to(&edges, &FlowTarget::Sink(code)), sunk, "{code}: {value}");
                let (params, user) = if code == "OD716" {
                    (Params::Route(&[]), "route")
                } else {
                    (Params::Rpc, "RPC")
                };
                let (found, edges) = flows_of(params, &src);
                assert_eq!(found, [code], "{code}: {user} {value}");
                assert!(
                    origins_to(&edges, &FlowTarget::Sink(code)).is_empty(),
                    "{code}: {user} {value}"
                );
            }
        }
        for value in ["a or self._x()", "self._x() or a"] {
            let src = format!("def _f(self, a):\n    self.write({value})\n");
            let (_, edges) = flows_of(Params::None, &src);
            assert_eq!(origins_to(&edges, &FlowTarget::SelfWrite), ["Param(0)"], "{value}");
            let (found, edges) = flows_of(Params::Rpc, &src);
            assert!(found.is_empty(), "RPC {value}");
            assert_eq!(origins_to(&edges, &FlowTarget::SelfWrite), ["Rpc(0)"], "RPC {value}");
        }
    }

    #[test]
    fn a_bound_condition_blames_the_value_harmless_for_fewer_rules() {
        let src = "def _set(self, field, value):\n    if field not in self._fields:\n        raise ValueError()\n    args = ('t', field, value)\n    self.env.cr.execute(\"UPDATE %s SET %s = '%s'\" % args)\n";
        let (_, edges) = flows_of(Params::None, src);
        assert_eq!(origins_to(&edges, &FlowTarget::Sink("OD701")), ["Param(1)"]);
    }

    #[test]
    fn an_attribute_set_on_a_returned_value_keeps_only_a_user_value() {
        for set in [
            "stream.etag += f'{crop}'",
            "stream.etag = stream.etag + f'{crop}'",
            "stream.etag = f'{crop}'",
        ] {
            let src = format!("def _f(self, crop):\n    stream = self._s()\n    {set}\n    open(stream.path)\n");
            let (_, edges) = flows_of(Params::None, &src);
            assert_eq!(origins_to(&edges, &FlowTarget::Sink("OD707")), ["Return(0)"], "{set}");
            let user = set.replace("f'{crop}'", "request.params['x']");
            let src = format!("def _f(self):\n    stream = self._s()\n    {user}\n    open(stream.path)\n");
            assert_eq!(flows_of(Params::None, &src).0, ["OD707"], "{user}");
        }
    }

    #[test]
    fn a_held_join_a_helper_never_checks_keeps_the_edge_for_its_callers() {
        let sinks = |body: &str| {
            let helper = summary(Params::None, &format!("def _open(self, name):\n{body}\n"));
            let edges = helper.edges.iter().filter(|e| e.target == FlowTarget::Sink("OD707"));
            edges.map(|e| e.origin).collect::<Vec<_>>()
        };
        let p = "    p = os.path.normpath(os.path.join('/srv', name))\n";
        assert_eq!(sinks(&format!("{p}    open(p)")), [FlowOrigin::Param(0)]);
        assert!(
            sinks(&format!(
                "{p}    if p.startswith('/srv/'):\n        open(p)\n    return name"
            ))
            .is_empty()
        );
        let helper = summary(
            Params::None,
            "def _open(self, name):\n    p = os.path.join('/srv', name)\n    open(name)\n",
        );
        let sink = helper.edges.iter().find(|e| e.target == FlowTarget::Sink("OD707"));
        let at = sink
            .and_then(|e| e.steps.last())
            .map(|s| (s.value.as_str(), s.pos.line));
        assert_eq!(at, Some(("os.path.join()", 2)));
    }
}
